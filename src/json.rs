use pyo3::prelude::*;
use pyo3::types::{
    PyBool, PyByteArray, PyBytes, PyDict, PyFloat, PyFrozenSet, PyInt, PyList, PyMapping, PyNone,
    PySet, PyString, PyTuple,
};
use std::collections::HashSet;
use std::fmt::{self, Write as _};

const MAX_DEPTH: usize = 256;
const SIGNAL_CHECK_INTERVAL: usize = 1000;

#[derive(Debug)]
pub(crate) enum ErrorReason {
    CircularReference,
    MaxDepthExceeded(usize),
    UnsupportedType(String),
    UnsupportedDictKey(String),
    InvalidFloat(f64),
    // Carries the original Python exception so callers can inspect or re-raise
    // it with full traceback intact (e.g. KeyboardInterrupt, ValueError from a
    // generator, TypeError from a custom Mapping).
    PythonError(PyErr),
}

impl From<PyErr> for ErrorReason {
    fn from(err: PyErr) -> Self {
        ErrorReason::PythonError(err)
    }
}

/// Structured error carrying the precise JSON path where failure occurred.
#[derive(Debug)]
pub(crate) struct PyJsonError {
    path: String,
    reason: ErrorReason,
}

impl fmt::Display for PyJsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.reason {
            ErrorReason::CircularReference => {
                write!(f, "Circular reference detected at {}", self.path)
            }
            ErrorReason::MaxDepthExceeded(d) => {
                write!(
                    f,
                    "Maximum nesting depth exceeded at {}: {} > {}",
                    self.path, d, MAX_DEPTH
                )
            }
            ErrorReason::UnsupportedType(t) => {
                write!(
                    f,
                    "Cannot serialize Python type to JSON at {}: {}",
                    self.path, t
                )
            }
            ErrorReason::UnsupportedDictKey(t) => {
                write!(f, "Unsupported dict key type at {}: {}", self.path, t)
            }
            ErrorReason::InvalidFloat(v) => {
                write!(
                    f,
                    "JSON does not support NaN or Infinity at {}: {}",
                    self.path, v
                )
            }
            ErrorReason::PythonError(e) => {
                write!(f, "Python exception at {}: {}", self.path, e)
            }
        }
    }
}

impl std::error::Error for PyJsonError {}

/// Re-raise as the original Python exception when possible, otherwise wrap in
/// TypeError. Lets call sites do `map_err(PyErr::from)?` without losing the
/// original traceback.
impl From<PyJsonError> for PyErr {
    fn from(err: PyJsonError) -> Self {
        match err.reason {
            ErrorReason::PythonError(py_err) => py_err,
            _ => pyo3::exceptions::PyTypeError::new_err(err.to_string()),
        }
    }
}

fn type_name_of(obj: &Bound<'_, pyo3::PyAny>) -> String {
    obj.get_type()
        .name()
        .map_or_else(|_| "<unknown>".to_string(), |n| n.to_string())
}

pub(crate) struct JsonContext {
    // Raw pointer used directly — Rust implements Hash+Eq for *mut T via
    // address, which is exactly the object-identity semantics we need.
    visited: HashSet<*mut pyo3::ffi::PyObject>,
    // Single reusable buffer: segments are appended on descent and truncated
    // on ascent. One allocation for the entire serialization, zero per-key
    // clones on the hot path.
    path_buffer: String,
    depth: usize,
    element_count: usize,
}

impl JsonContext {
    fn new() -> Self {
        Self {
            visited: HashSet::new(),
            path_buffer: String::from("$"),
            depth: 0,
            element_count: 0,
        }
    }

    fn err(&self, reason: ErrorReason) -> PyJsonError {
        PyJsonError {
            path: self.path_buffer.clone(),
            reason,
        }
    }

    fn maybe_check_signals(&mut self, py: Python<'_>) -> Result<(), PyJsonError> {
        self.element_count += 1;
        if self.element_count.is_multiple_of(SIGNAL_CHECK_INTERVAL) {
            py.check_signals().map_err(|e| self.err(e.into()))?;
        }
        Ok(())
    }

    fn serialize(
        &mut self,
        obj: &Bound<'_, pyo3::PyAny>,
    ) -> Result<serde_json::Value, PyJsonError> {
        if self.depth > MAX_DEPTH {
            return Err(self.err(ErrorReason::MaxDepthExceeded(self.depth)));
        }

        let ptr = obj.as_ptr();
        if !self.visited.insert(ptr) {
            return Err(self.err(ErrorReason::CircularReference));
        }

        let result = self.parse_node(obj);

        self.visited.remove(&ptr);
        result
    }

    fn parse_node(
        &mut self,
        obj: &Bound<'_, pyo3::PyAny>,
    ) -> Result<serde_json::Value, PyJsonError> {
        if obj.is_none() {
            return Ok(serde_json::Value::Null);
        }

        // String first — most common leaf type in JSON payloads; also must
        // precede the iterable fallback (str implements Sequence in Python).
        if let Ok(s) = obj.cast::<PyString>() {
            return match s.to_str() {
                Ok(str_ref) => Ok(serde_json::Value::String(str_ref.to_owned())),
                Err(e) => Err(self.err(e.into())),
            };
        }

        // Fast path: PyDict
        if let Ok(dict) = obj.cast::<PyDict>() {
            return self.serialize_dict_pairs(dict.iter(), dict.len(), obj.py());
        }

        // Fast path: PyList
        if let Ok(list) = obj.cast::<PyList>() {
            return self.serialize_seq(list.iter().map(Ok), list.len(), obj.py());
        }

        // cast_exact: match PyBool before PyInt to prevent subclass confusion
        if let Ok(b) = obj.cast_exact::<PyBool>() {
            return Ok(serde_json::Value::Bool(b.is_true()));
        }

        if let Ok(i) = obj.cast::<PyInt>() {
            // i64 covers the common case (signed 64-bit).
            if let Ok(v) = i.extract::<i64>() {
                return Ok(serde_json::Value::Number(v.into()));
            }
            // u64 covers positive values up to 2^64−1.
            if let Ok(v) = i.extract::<u64>() {
                return Ok(serde_json::Value::Number(v.into()));
            }
            // For integers beyond u64::MAX we convert to f64. Precision is
            // lost above 2^53, but the value stays a JSON Number rather than
            // being silently coerced to a String (wrong type for downstream).
            // Enabling serde_json's `arbitrary_precision` feature would give
            // exact round-trip, but allocates a heap String for every Number
            // — too expensive at 400k+ req/s for the common i64/u64 case.
            let fv = i.extract::<f64>().unwrap_or(f64::MAX);
            if let Some(n) = serde_json::Number::from_f64(fv) {
                return Ok(serde_json::Value::Number(n));
            }
            return Err(self.err(ErrorReason::UnsupportedType(format!(
                "int value not representable as JSON number: {}",
                i.to_string()
            ))));
        }

        if let Ok(f) = obj.cast::<PyFloat>() {
            let v = f.value();
            if v.is_nan() || v.is_infinite() {
                return Err(self.err(ErrorReason::InvalidFloat(v)));
            }
            if let Some(n) = serde_json::Number::from_f64(v) {
                return Ok(serde_json::Value::Number(n));
            }
        }

        // Fast path: PyTuple
        if let Ok(tuple) = obj.cast::<PyTuple>() {
            return self.serialize_seq(tuple.iter().map(Ok), tuple.len(), obj.py());
        }

        // Reject bytes/bytearray — they implement Sequence but must not become int arrays
        if obj.cast::<PyBytes>().is_ok() || obj.cast::<PyByteArray>().is_ok() {
            return Err(self.err(ErrorReason::UnsupportedType(type_name_of(obj))));
        }

        // Reject set/frozenset explicitly. Without this guard the duck-type
        // iterable fallback below would silently produce a JSON array with
        // non-deterministic ordering. stdlib json.dumps raises TypeError for
        // sets; surface the bug to the developer instead of hiding it.
        if obj.cast::<PySet>().is_ok() || obj.cast::<PyFrozenSet>().is_ok() {
            return Err(self.err(ErrorReason::UnsupportedType(type_name_of(obj))));
        }

        // Duck type: PyMapping (defaultdict, OrderedDict, custom Mapping subclasses)
        if let Ok(mapping) = obj.cast::<PyMapping>() {
            match mapping.items() {
                Ok(items) => {
                    let len = mapping.len().unwrap_or(0);
                    return self.serialize_mapping_items(&items, len, obj.py());
                }
                Err(e) => return Err(self.err(e.into())),
            }
        }

        // Duck type: any iterable (deque, generators, etc.) — O(1) per step.
        // Snapshot path before the iterator so the closure doesn't borrow
        // `self` while `serialize_seq` holds `&mut self`.
        if let Ok(iter) = obj.try_iter() {
            let err_path = self.path_buffer.clone();
            return self.serialize_seq(
                iter.map(move |r| {
                    r.map_err(|e| PyJsonError {
                        path: err_path.clone(),
                        reason: e.into(),
                    })
                }),
                0,
                obj.py(),
            );
        }

        Err(self.err(ErrorReason::UnsupportedType(type_name_of(obj))))
    }

    /// Serialize an iterator of (key, value) pairs into a JSON object.
    fn serialize_dict_pairs<'py>(
        &mut self,
        iter: impl Iterator<Item = (Bound<'py, pyo3::PyAny>, Bound<'py, pyo3::PyAny>)>,
        capacity: usize,
        py: Python<'py>,
    ) -> Result<serde_json::Value, PyJsonError> {
        let mut map = serde_json::Map::with_capacity(capacity);
        for (k, v) in iter {
            self.maybe_check_signals(py)?;
            let key_str = self.coerce_dict_key(&k)?;

            // Append the key segment, serialize the value, then truncate back.
            // key_str is not moved into the buffer — only its chars are written —
            // so it can be moved directly into the map after truncate: zero clones.
            let orig = self.path_buffer.len();
            write!(&mut self.path_buffer, ".{}", key_str).ok();
            self.depth += 1;
            let val = self.serialize(&v);
            self.path_buffer.truncate(orig);
            self.depth -= 1;

            map.insert(key_str, val?);
        }
        Ok(serde_json::Value::Object(map))
    }

    /// Serialize items() from a generic PyMapping (list of 2-tuples).
    fn serialize_mapping_items<'py>(
        &mut self,
        items: &Bound<'py, PyList>,
        capacity: usize,
        py: Python<'py>,
    ) -> Result<serde_json::Value, PyJsonError> {
        let mut map = serde_json::Map::with_capacity(capacity);
        for item in items.iter() {
            self.maybe_check_signals(py)?;

            let k = item.get_item(0).map_err(|e| self.err(e.into()))?;
            let v = item.get_item(1).map_err(|e| self.err(e.into()))?;

            let key_str = self.coerce_dict_key(&k)?;

            let orig = self.path_buffer.len();
            write!(&mut self.path_buffer, ".{}", key_str).ok();
            self.depth += 1;
            let val = self.serialize(&v);
            self.path_buffer.truncate(orig);
            self.depth -= 1;

            map.insert(key_str, val?);
        }
        Ok(serde_json::Value::Object(map))
    }

    /// Serialize a fallible sequence iterator into a JSON array.
    /// Infallible iterators (PyList, PyTuple) pass `.map(Ok)`.
    fn serialize_seq<'py>(
        &mut self,
        iter: impl Iterator<Item = Result<Bound<'py, pyo3::PyAny>, PyJsonError>>,
        capacity: usize,
        py: Python<'py>,
    ) -> Result<serde_json::Value, PyJsonError> {
        let mut arr = Vec::with_capacity(capacity);
        for (idx, item_result) in iter.enumerate() {
            self.maybe_check_signals(py)?;
            let item = item_result?;

            let orig = self.path_buffer.len();
            write!(&mut self.path_buffer, "[{}]", idx).ok();
            self.depth += 1;
            let val = self.serialize(&item);
            self.path_buffer.truncate(orig);
            self.depth -= 1;

            arr.push(val?);
        }
        Ok(serde_json::Value::Array(arr))
    }

    /// Coerce a Python dict key to a JSON string.
    /// Supports: str, bool, int, float, None (matching Python json.dumps).
    fn coerce_dict_key(&self, k: &Bound<'_, pyo3::PyAny>) -> Result<String, PyJsonError> {
        if let Ok(py_str) = k.cast::<PyString>() {
            return match py_str.to_str() {
                Ok(s) => Ok(s.to_owned()),
                Err(e) => Err(self.err(e.into())),
            };
        }

        // bool before int (bool is int subclass in Python)
        if let Ok(b) = k.cast_exact::<PyBool>() {
            return Ok(if b.is_true() {
                "true".to_string()
            } else {
                "false".to_string()
            });
        }

        if k.cast::<PyInt>().is_ok() {
            return Ok(k.to_string());
        }

        if let Ok(f) = k.cast::<PyFloat>() {
            let fv = f.value();
            if fv.is_nan() || fv.is_infinite() {
                return Err(self.err(ErrorReason::InvalidFloat(fv)));
            }
            // Match Python json.dumps: whole-number floats always include the
            // decimal point (1.0 → "1.0", not "1" as Rust's Display produces).
            let s = fv.to_string();
            return Ok(if s.contains('.') || s.contains('e') || s.contains('E') {
                s
            } else {
                format!("{s}.0")
            });
        }

        if k.cast::<PyNone>().is_ok() {
            return Ok("null".to_string());
        }

        Err(self.err(ErrorReason::UnsupportedDictKey(type_name_of(k))))
    }
}

/// Convert a Python object to a serde_json::Value.
/// Errors include JSON path context (e.g. `$.users[2].score`).
pub(crate) fn py_to_json_value(
    obj: &pyo3::Bound<'_, pyo3::PyAny>,
) -> Result<serde_json::Value, PyJsonError> {
    let mut ctx = JsonContext::new();
    ctx.serialize(obj)
}
