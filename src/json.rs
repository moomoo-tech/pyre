use bytes::Bytes;
use parking_lot::Mutex;
use pyo3::prelude::*;
use pyo3::types::{
    PyBool, PyByteArray, PyBytes, PyDict, PyFloat, PyFrozenSet, PyInt, PyList, PyMapping, PyNone,
    PySet, PyString, PyTuple,
};
use std::collections::HashSet;
use std::fmt;

const MAX_DEPTH: usize = 256;
const SIGNAL_CHECK_INTERVAL: usize = 1000;
const HEX: &[u8; 16] = b"0123456789abcdef";

/// Compile-time table: 1 for every byte that must be escaped in a JSON string.
/// Fits in 256 bytes (< one cache line cluster). LLVM uses this as a gather
/// source for its auto-vectorized scan of the input string.
const fn build_escape_table() -> [u8; 256] {
    let mut t = [0u8; 256];
    let mut i = 0u8;
    while i < 0x20 {
        t[i as usize] = 1; // C0 control characters
        i += 1;
    }
    t[b'"' as usize] = 1;
    t[b'\\' as usize] = 1;
    t
}
static ESCAPE_TABLE: [u8; 256] = build_escape_table();

// ---------------------------------------------------------------------------
// Global output-buffer pool
// ---------------------------------------------------------------------------

// parking_lot::Mutex is const-constructible and uncontended at ~ns cost.
// Bounded at 64 buffers (≈ 2× typical worker-thread count) so memory usage
// is bounded; buffers over 1 MiB are discarded rather than hoarded.
static BUFFER_POOL: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());

/// `Vec<u8>` wrapper that returns its buffer to `BUFFER_POOL` on drop.
/// Wrapped in `Bytes::from_owner(...)` so the buffer lives until the HTTP
/// layer finishes writing — then returns without an extra memcpy.
struct PooledVec(Vec<u8>);

impl AsRef<[u8]> for PooledVec {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl Drop for PooledVec {
    fn drop(&mut self) {
        let mut v = std::mem::take(&mut self.0);
        if v.capacity() <= 1 << 20 {
            v.clear();
            let mut pool = BUFFER_POOL.lock();
            if pool.len() < 64 {
                pool.push(v);
            }
        }
    }
}

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

/// Write a JSON-escaped, double-quoted string directly into `out`.
///
/// The inner loop uses `Iterator::position` over a 256-byte lookup table so
/// LLVM can auto-vectorize the scan with SSE2/AVX2 `pcmpeqb`/`pshufb` when
/// the target supports it — no unsafe code needed.
fn write_str_escaped(out: &mut Vec<u8>, s: &str) {
    out.push(b'"');
    let bytes = s.as_bytes();
    let mut start = 0;
    loop {
        // Bulk-copy the next run of bytes that need no escaping.
        // LLVM recognizes the table-lookup predicate and emits a vectorized scan.
        let end = bytes[start..]
            .iter()
            .position(|&b| ESCAPE_TABLE[b as usize] != 0)
            .map_or(bytes.len(), |p| start + p);
        out.extend_from_slice(&bytes[start..end]);
        if end == bytes.len() {
            break;
        }
        let byte = bytes[end];
        match byte {
            b'"' => out.extend_from_slice(b"\\\""),
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'\x08' => out.extend_from_slice(b"\\b"),
            b'\x0c' => out.extend_from_slice(b"\\f"),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            b'\t' => out.extend_from_slice(b"\\t"),
            _ => out.extend_from_slice(&[
                b'\\',
                b'u',
                b'0',
                b'0',
                HEX[(byte >> 4) as usize],
                HEX[(byte & 0xf) as usize],
            ]),
        }
        start = end + 1;
    }
    out.push(b'"');
}

pub(crate) struct JsonContext {
    // Raw pointer used directly — Rust implements Hash+Eq for *mut T via
    // address, which is exactly the object-identity semantics we need.
    visited: HashSet<*mut pyo3::ffi::PyObject>,
    // Single reusable buffer for error paths. Appended on descent, truncated
    // on ascent — zero per-key clones on the hot path.
    path_buffer: String,
    depth: usize,
    // Countdown to next signal check. Counts down from SIGNAL_CHECK_INTERVAL
    // to 0; a compare-to-zero is cheaper than the previous modulo.
    signal_countdown: usize,
    // Output buffer written directly — eliminates the serde_json::Value
    // intermediate tree and the separate sonic_rs serialization pass.
    out: Vec<u8>,
}

impl JsonContext {
    fn new() -> Self {
        Self {
            visited: HashSet::new(),
            path_buffer: String::from("$"),
            depth: 0,
            signal_countdown: SIGNAL_CHECK_INTERVAL,
            out: Vec::with_capacity(512),
        }
    }

    fn new_with_buf(buf: Vec<u8>) -> Self {
        Self {
            visited: HashSet::new(),
            path_buffer: String::from("$"),
            depth: 0,
            signal_countdown: SIGNAL_CHECK_INTERVAL,
            out: buf,
        }
    }

    fn into_buf(self) -> Vec<u8> {
        self.out
    }

    fn err(&self, reason: ErrorReason) -> PyJsonError {
        PyJsonError {
            path: self.path_buffer.clone(),
            reason,
        }
    }

    fn maybe_check_signals(&mut self, py: Python<'_>) -> Result<(), PyJsonError> {
        self.signal_countdown -= 1;
        if self.signal_countdown == 0 {
            self.signal_countdown = SIGNAL_CHECK_INTERVAL;
            py.check_signals().map_err(|e| self.err(e.into()))?;
        }
        Ok(())
    }

    fn serialize(&mut self, obj: &Bound<'_, pyo3::PyAny>) -> Result<(), PyJsonError> {
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

    fn parse_node(&mut self, obj: &Bound<'_, pyo3::PyAny>) -> Result<(), PyJsonError> {
        if obj.is_none() {
            self.out.extend_from_slice(b"null");
            return Ok(());
        }

        // String first — most common leaf type in JSON payloads; also must
        // precede the iterable fallback (str implements Sequence in Python).
        if let Ok(s) = obj.cast::<PyString>() {
            return match s.to_str() {
                Ok(str_ref) => {
                    write_str_escaped(&mut self.out, str_ref);
                    Ok(())
                }
                Err(e) => Err(self.err(e.into())),
            };
        }

        // Fast path: PyDict
        if let Ok(dict) = obj.cast::<PyDict>() {
            self.out.reserve(dict.len().saturating_mul(16));
            return self.serialize_dict(dict.iter(), obj.py());
        }

        // Fast path: PyList
        if let Ok(list) = obj.cast::<PyList>() {
            self.out.reserve(list.len().saturating_mul(8));
            return self.serialize_array(list.iter().map(Ok), obj.py());
        }

        // cast_exact: match PyBool before PyInt to prevent subclass confusion
        if let Ok(b) = obj.cast_exact::<PyBool>() {
            self.out
                .extend_from_slice(if b.is_true() { b"true" } else { b"false" });
            return Ok(());
        }

        if let Ok(i) = obj.cast::<PyInt>() {
            // i64 covers the common case (signed 64-bit).
            if let Ok(v) = i.extract::<i64>() {
                let mut buf = itoa::Buffer::new();
                self.out.extend_from_slice(buf.format(v).as_bytes());
                return Ok(());
            }
            // u64 covers positive values up to 2^64−1.
            if let Ok(v) = i.extract::<u64>() {
                let mut buf = itoa::Buffer::new();
                self.out.extend_from_slice(buf.format(v).as_bytes());
                return Ok(());
            }
            // For integers beyond u64::MAX we convert to f64. Precision is
            // lost above 2^53, but the value stays a JSON Number rather than
            // being silently coerced to a String (wrong type for downstream).
            let fv = i.extract::<f64>().unwrap_or(f64::MAX);
            if fv.is_finite() {
                let mut buf = ryu::Buffer::new();
                self.out.extend_from_slice(buf.format_finite(fv).as_bytes());
                return Ok(());
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
            let mut buf = ryu::Buffer::new();
            self.out.extend_from_slice(buf.format_finite(v).as_bytes());
            return Ok(());
        }

        // Fast path: PyTuple
        if let Ok(tuple) = obj.cast::<PyTuple>() {
            self.out.reserve(tuple.len().saturating_mul(8));
            return self.serialize_array(tuple.iter().map(Ok), obj.py());
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
                Ok(items) => return self.serialize_mapping(&items, obj.py()),
                Err(e) => return Err(self.err(e.into())),
            }
        }

        // Duck type: any iterable (deque, generators, etc.) — O(1) per step.
        // Snapshot path before the iterator so the closure doesn't borrow
        // `self` while `serialize_array` holds `&mut self`.
        if let Ok(iter) = obj.try_iter() {
            let err_path = self.path_buffer.clone();
            return self.serialize_array(
                iter.map(move |r| {
                    r.map_err(|e| PyJsonError {
                        path: err_path.clone(),
                        reason: e.into(),
                    })
                }),
                obj.py(),
            );
        }

        Err(self.err(ErrorReason::UnsupportedType(type_name_of(obj))))
    }

    /// Serialize a PyDict iterator directly to `{...}` bytes.
    fn serialize_dict<'py>(
        &mut self,
        iter: impl Iterator<Item = (Bound<'py, pyo3::PyAny>, Bound<'py, pyo3::PyAny>)>,
        py: Python<'py>,
    ) -> Result<(), PyJsonError> {
        self.out.push(b'{');
        let mut first = true;
        for (k, v) in iter {
            self.maybe_check_signals(py)?;
            let key_str = self.coerce_dict_key(&k)?;

            if !first {
                self.out.push(b',');
            }
            first = false;

            write_str_escaped(&mut self.out, &key_str);
            self.out.push(b':');

            let orig = self.path_buffer.len();
            self.path_buffer.push('.');
            self.path_buffer.push_str(&key_str);
            self.depth += 1;
            let result = self.serialize(&v);
            self.path_buffer.truncate(orig);
            self.depth -= 1;
            result?;
        }
        self.out.push(b'}');
        Ok(())
    }

    /// Serialize items() from a generic PyMapping (list of 2-tuples) to `{...}`.
    fn serialize_mapping<'py>(
        &mut self,
        items: &Bound<'py, PyList>,
        py: Python<'py>,
    ) -> Result<(), PyJsonError> {
        self.out.reserve(items.len().saturating_mul(16));
        self.out.push(b'{');
        let mut first = true;
        for item in items.iter() {
            self.maybe_check_signals(py)?;

            let k = item.get_item(0).map_err(|e| self.err(e.into()))?;
            let v = item.get_item(1).map_err(|e| self.err(e.into()))?;
            let key_str = self.coerce_dict_key(&k)?;

            if !first {
                self.out.push(b',');
            }
            first = false;

            write_str_escaped(&mut self.out, &key_str);
            self.out.push(b':');

            let orig = self.path_buffer.len();
            self.path_buffer.push('.');
            self.path_buffer.push_str(&key_str);
            self.depth += 1;
            let result = self.serialize(&v);
            self.path_buffer.truncate(orig);
            self.depth -= 1;
            result?;
        }
        self.out.push(b'}');
        Ok(())
    }

    /// Serialize a fallible sequence iterator to `[...]` bytes.
    /// Infallible iterators (PyList, PyTuple) pass `.map(Ok)`.
    fn serialize_array<'py>(
        &mut self,
        iter: impl Iterator<Item = Result<Bound<'py, pyo3::PyAny>, PyJsonError>>,
        py: Python<'py>,
    ) -> Result<(), PyJsonError> {
        self.out.push(b'[');
        let mut first = true;
        for (idx, item_result) in iter.enumerate() {
            self.maybe_check_signals(py)?;
            let item = item_result?;

            if !first {
                self.out.push(b',');
            }
            first = false;

            let orig = self.path_buffer.len();
            self.path_buffer.push('[');
            let mut ibuf = itoa::Buffer::new();
            self.path_buffer.push_str(ibuf.format(idx));
            self.path_buffer.push(']');
            self.depth += 1;
            let result = self.serialize(&item);
            self.path_buffer.truncate(orig);
            self.depth -= 1;
            result?;
        }
        self.out.push(b']');
        Ok(())
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

/// Serialize a Python object directly to JSON bytes.
///
/// Grabs a pre-warmed `Vec<u8>` from `BUFFER_POOL` (zero allocation after
/// the first few requests), writes JSON in a single pass, then wraps the
/// buffer in `Bytes::from_owner` so it is returned to the pool when the HTTP
/// layer drops the response body — true zero-copy, zero per-request malloc.
pub(crate) fn py_to_json_bytes(
    obj: &pyo3::Bound<'_, pyo3::PyAny>,
) -> Result<Bytes, PyJsonError> {
    let buf = BUFFER_POOL.lock().pop().unwrap_or_else(|| Vec::with_capacity(4096));
    let mut ctx = JsonContext::new_with_buf(buf);
    ctx.serialize(obj)?;
    Ok(Bytes::from_owner(PooledVec(ctx.into_buf())))
}
