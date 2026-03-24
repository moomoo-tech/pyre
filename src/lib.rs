use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use matchit::Router;
use parking_lot::RwLock;
use pyo3::ffi;
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyDict, PyFloat, PyInt, PyList, PyString};
use tokio::net::TcpListener;
use tokio::runtime::Builder as RuntimeBuilder;
use tokio::signal;

// ---------------------------------------------------------------------------
// Request object exposed to Python handlers
// ---------------------------------------------------------------------------

#[pyclass(frozen)]
#[derive(Clone)]
struct SkyRequest {
    #[pyo3(get)]
    method: String,
    #[pyo3(get)]
    path: String,
    #[pyo3(get)]
    params: HashMap<String, String>,
    #[pyo3(get)]
    query: String,
    #[pyo3(get)]
    headers: HashMap<String, String>,
    body_bytes: Vec<u8>,
}

#[pymethods]
impl SkyRequest {
    #[getter]
    fn body(&self) -> &[u8] {
        &self.body_bytes
    }

    fn text(&self) -> PyResult<String> {
        String::from_utf8(self.body_bytes.clone())
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))
    }

    fn json<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, pyo3::PyAny>> {
        let text = self.text()?;
        let json_mod = py.import("json")?;
        json_mod.call_method1("loads", (text,))
    }

    #[getter]
    fn query_params(&self) -> HashMap<String, String> {
        form_urlencoded::parse(self.query.as_bytes())
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Response object for custom status, headers, content-type
// ---------------------------------------------------------------------------

#[pyclass(frozen)]
struct SkyResponse {
    #[pyo3(get)]
    body: Py<PyAny>,
    #[pyo3(get)]
    status_code: u16,
    #[pyo3(get)]
    content_type: Option<String>,
    #[pyo3(get)]
    headers: HashMap<String, String>,
}

#[pymethods]
impl SkyResponse {
    #[new]
    #[pyo3(signature = (body, status_code=200, content_type=None, headers=None))]
    fn new(
        body: Py<PyAny>,
        status_code: u16,
        content_type: Option<String>,
        headers: Option<HashMap<String, String>>,
    ) -> Self {
        SkyResponse {
            body,
            status_code,
            content_type,
            headers: headers.unwrap_or_default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Rust-side JSON serialization
// ---------------------------------------------------------------------------

fn py_to_json_value(obj: &Bound<'_, pyo3::PyAny>) -> Result<serde_json::Value, String> {
    if obj.is_none() {
        return Ok(serde_json::Value::Null);
    }
    if let Ok(b) = obj.downcast::<PyBool>() {
        return Ok(serde_json::Value::Bool(b.is_true()));
    }
    if let Ok(i) = obj.downcast::<PyInt>() {
        if let Ok(v) = i.extract::<i64>() {
            return Ok(serde_json::Value::Number(v.into()));
        }
    }
    if let Ok(f) = obj.downcast::<PyFloat>() {
        if let Ok(v) = f.extract::<f64>() {
            if let Some(n) = serde_json::Number::from_f64(v) {
                return Ok(serde_json::Value::Number(n));
            }
        }
    }
    if let Ok(s) = obj.downcast::<PyString>() {
        return Ok(serde_json::Value::String(s.to_string()));
    }
    if let Ok(list) = obj.downcast::<PyList>() {
        let mut arr = Vec::with_capacity(list.len());
        for item in list.iter() {
            arr.push(py_to_json_value(&item)?);
        }
        return Ok(serde_json::Value::Array(arr));
    }
    if let Ok(dict) = obj.downcast::<PyDict>() {
        let mut map = serde_json::Map::with_capacity(dict.len());
        for (k, v) in dict.iter() {
            let key = k.extract::<String>().map_err(|e| e.to_string())?;
            map.insert(key, py_to_json_value(&v)?);
        }
        return Ok(serde_json::Value::Object(map));
    }
    Ok(serde_json::Value::String(
        obj.str().map_err(|e| e.to_string())?.to_string(),
    ))
}

// ---------------------------------------------------------------------------
// Route table (main interpreter mode)
// ---------------------------------------------------------------------------

struct RouteTable {
    handlers: Vec<Py<PyAny>>,
    handler_names: Vec<String>,
    routers: HashMap<String, Router<usize>>,
    before_hooks: Vec<Py<PyAny>>,
    after_hooks: Vec<Py<PyAny>>,
    before_hook_names: Vec<String>,
    after_hook_names: Vec<String>,
    fallback_handler: Option<Py<PyAny>>,
    fallback_handler_name: Option<String>,
    static_dirs: Vec<(String, String)>, // (url_prefix, fs_directory)
}

impl RouteTable {
    fn new() -> Self {
        RouteTable {
            handlers: Vec::new(),
            handler_names: Vec::new(),
            routers: HashMap::new(),
            before_hooks: Vec::new(),
            after_hooks: Vec::new(),
            before_hook_names: Vec::new(),
            after_hook_names: Vec::new(),
            fallback_handler: None,
            fallback_handler_name: None,
            static_dirs: Vec::new(),
        }
    }

    fn insert(
        &mut self,
        method: &str,
        path: &str,
        handler: Py<PyAny>,
        handler_name: String,
    ) -> Result<(), String> {
        let idx = self.handlers.len();
        self.handlers.push(handler);
        self.handler_names.push(handler_name);
        let router = self
            .routers
            .entry(method.to_uppercase())
            .or_insert_with(Router::new);
        router.insert(path, idx).map_err(|e| e.to_string())?;
        Ok(())
    }

    fn lookup(&self, method: &str, path: &str) -> Option<(usize, HashMap<String, String>)> {
        let router = self.routers.get(method)?;
        let matched = router.at(path).ok()?;
        let params: HashMap<String, String> = matched
            .params
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Some((*matched.value, params))
    }
}

unsafe impl Send for RouteTable {}
unsafe impl Sync for RouteTable {}

type SharedRoutes = Arc<RwLock<RouteTable>>;

// ---------------------------------------------------------------------------
// Response data (shared between GIL and sub-interpreter paths)
// ---------------------------------------------------------------------------

struct ResponseData {
    body: Bytes,
    content_type: String,
    status: u16,
    headers: HashMap<String, String>,
}

// ---------------------------------------------------------------------------
// Sub-interpreter worker pool
// ---------------------------------------------------------------------------

struct SubInterpreter {
    tstate: *mut ffi::PyThreadState,
}

unsafe impl Send for SubInterpreter {}
unsafe impl Sync for SubInterpreter {}

struct InterpreterPool {
    workers: Vec<SubInterpreter>,
    locks: Vec<parking_lot::Mutex<()>>,
    counter: AtomicUsize,
    handler_maps: Vec<HashMap<String, *mut ffi::PyObject>>,
    globals: Vec<*mut ffi::PyObject>,
    routers: HashMap<String, Router<usize>>,
    handler_names: Vec<String>,
    before_hook_names: Vec<String>,
    #[allow(dead_code)]
    after_hook_names: Vec<String>,
    static_dirs: Vec<(String, String)>,
}

unsafe impl Send for InterpreterPool {}
unsafe impl Sync for InterpreterPool {}

/// Result from sub-interpreter handler call
struct SubInterpResponse {
    body: String,
    status: u16,
    content_type: Option<String>,
    headers: HashMap<String, String>,
    is_json: bool,
}

impl InterpreterPool {
    unsafe fn new(
        n: usize,
        script_path: &str,
        handler_names: &[String],
        routers: HashMap<String, Router<usize>>,
        before_hook_names: &[String],
        after_hook_names: &[String],
        static_dirs: Vec<(String, String)>,
    ) -> Result<Self, String> {
        let mut workers = Vec::with_capacity(n);
        let mut handler_maps = Vec::with_capacity(n);
        let mut all_globals = Vec::with_capacity(n);

        let main_tstate = ffi::PyThreadState_Get();

        // Collect all function names we need to extract
        let mut all_names: Vec<String> = handler_names.to_vec();
        all_names.extend(before_hook_names.iter().cloned());
        all_names.extend(after_hook_names.iter().cloned());

        for i in 0..n {
            let mut new_tstate: *mut ffi::PyThreadState = std::ptr::null_mut();
            let config = ffi::PyInterpreterConfig {
                use_main_obmalloc: 0,
                allow_fork: 0,
                allow_exec: 0,
                allow_threads: 1,
                allow_daemon_threads: 0,
                check_multi_interp_extensions: 1,
                gil: ffi::PyInterpreterConfig_OWN_GIL,
            };

            let status = ffi::Py_NewInterpreterFromConfig(&mut new_tstate, &config);
            if ffi::PyStatus_IsError(status) != 0 || new_tstate.is_null() {
                ffi::PyThreadState_Swap(main_tstate);
                return Err(format!("Failed to create sub-interpreter {i}"));
            }

            let mut hmap = HashMap::new();

            let raw_script = std::fs::read_to_string(script_path)
                .map_err(|e| format!("Failed to read script: {e}"))?;

            let script: String = raw_script
                .lines()
                .filter(|line| {
                    let trimmed = line.trim();
                    if trimmed.starts_with("from skytrade")
                        || trimmed.starts_with("import skytrade")
                    {
                        return false;
                    }
                    if trimmed.starts_with("app = ") || trimmed.starts_with("app.") {
                        return false;
                    }
                    if trimmed.starts_with("if __name__") {
                        return false;
                    }
                    true
                })
                .collect::<Vec<&str>>()
                .join("\n");

            let bootstrap = format!(
                r#"
class _SkyRequest:
    def __init__(self, method, path, params, query, body_bytes, headers):
        self.method = method
        self.path = path
        self.params = params
        self.query = query
        self.body_bytes = body_bytes
        self.headers = headers
    @property
    def body(self):
        return self.body_bytes
    @property
    def query_params(self):
        from urllib.parse import parse_qs
        return {{k: v[0] for k, v in parse_qs(self.query).items()}}
    def text(self):
        return self.body_bytes.decode('utf-8') if isinstance(self.body_bytes, bytes) else str(self.body_bytes)
    def json(self):
        import json
        return json.loads(self.text())

class _SkyResponse:
    def __init__(self, body="", status_code=200, content_type=None, headers=None):
        self.body = body
        self.status_code = status_code
        self.content_type = content_type
        self.headers = headers or {{}}

# Execute user script
{}
"#,
                script
            );

            let globals = ffi::PyDict_New();
            let builtins = ffi::PyEval_GetBuiltins();
            ffi::PyDict_SetItemString(globals, c"__builtins__".as_ptr(), builtins);

            let code_cstr = std::ffi::CString::new(bootstrap.as_bytes())
                .map_err(|e| format!("CString error: {e}"))?;
            let _filename_cstr = std::ffi::CString::new(script_path)
                .map_err(|e| format!("CString error: {e}"))?;

            let result = ffi::PyRun_String(
                code_cstr.as_ptr(),
                ffi::Py_file_input.try_into().unwrap(),
                globals,
                globals,
            );

            if result.is_null() {
                ffi::PyErr_Print();
                ffi::Py_DECREF(globals);
                ffi::PyThreadState_Swap(main_tstate);
                return Err(format!(
                    "Failed to execute script in sub-interpreter {i}"
                ));
            }
            ffi::Py_DECREF(result);

            for name in &all_names {
                let name_cstr = std::ffi::CString::new(name.as_bytes())
                    .map_err(|e| format!("CString error: {e}"))?;
                let func = ffi::PyDict_GetItemString(globals, name_cstr.as_ptr());
                if !func.is_null() && ffi::PyCallable_Check(func) != 0 {
                    ffi::Py_INCREF(func);
                    hmap.insert(name.clone(), func);
                }
            }

            ffi::Py_INCREF(globals);
            let saved = ffi::PyEval_SaveThread();

            workers.push(SubInterpreter { tstate: saved });
            handler_maps.push(hmap);
            all_globals.push(globals);
        }

        ffi::PyThreadState_Swap(main_tstate);

        let locks = (0..n).map(|_| parking_lot::Mutex::new(())).collect();
        Ok(InterpreterPool {
            workers,
            locks,
            counter: AtomicUsize::new(0),
            handler_maps,
            globals: all_globals,
            routers,
            handler_names: handler_names.to_vec(),
            before_hook_names: before_hook_names.to_vec(),
            after_hook_names: after_hook_names.to_vec(),
            static_dirs,
        })
    }

    fn pick_worker(&self) -> usize {
        self.counter.fetch_add(1, Ordering::Relaxed) % self.workers.len()
    }

    fn lookup(&self, method: &str, path: &str) -> Option<(usize, HashMap<String, String>)> {
        let router = self.routers.get(method)?;
        let matched = router.at(path).ok()?;
        let params: HashMap<String, String> = matched
            .params
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Some((*matched.value, params))
    }

    /// Build a _SkyRequest object in the current sub-interpreter context
    unsafe fn build_request_obj(
        &self,
        worker_idx: usize,
        method: &str,
        path: &str,
        params: &HashMap<String, String>,
        query: &str,
        body: &[u8],
        headers: &HashMap<String, String>,
    ) -> *mut ffi::PyObject {
        let py_method = ffi::PyUnicode_FromStringAndSize(
            method.as_ptr() as *const _,
            method.len() as isize,
        );
        let py_path = ffi::PyUnicode_FromStringAndSize(
            path.as_ptr() as *const _,
            path.len() as isize,
        );

        let py_params = ffi::PyDict_New();
        for (k, v) in params {
            let pk = ffi::PyUnicode_FromStringAndSize(k.as_ptr() as *const _, k.len() as isize);
            let pv = ffi::PyUnicode_FromStringAndSize(v.as_ptr() as *const _, v.len() as isize);
            ffi::PyDict_SetItem(py_params, pk, pv);
            ffi::Py_DECREF(pk);
            ffi::Py_DECREF(pv);
        }

        let py_query = ffi::PyUnicode_FromStringAndSize(
            query.as_ptr() as *const _,
            query.len() as isize,
        );
        let py_body = ffi::PyBytes_FromStringAndSize(
            body.as_ptr() as *const _,
            body.len() as isize,
        );

        let py_headers = ffi::PyDict_New();
        for (k, v) in headers {
            let pk = ffi::PyUnicode_FromStringAndSize(k.as_ptr() as *const _, k.len() as isize);
            let pv = ffi::PyUnicode_FromStringAndSize(v.as_ptr() as *const _, v.len() as isize);
            ffi::PyDict_SetItem(py_headers, pk, pv);
            ffi::Py_DECREF(pk);
            ffi::Py_DECREF(pv);
        }

        let args = ffi::PyTuple_New(6);
        ffi::PyTuple_SetItem(args, 0, py_method);
        ffi::PyTuple_SetItem(args, 1, py_path);
        ffi::PyTuple_SetItem(args, 2, py_params);
        ffi::PyTuple_SetItem(args, 3, py_query);
        ffi::PyTuple_SetItem(args, 4, py_body);
        ffi::PyTuple_SetItem(args, 5, py_headers);

        let worker_globals = self.globals[worker_idx];
        let req_class_name = std::ffi::CString::new("_SkyRequest").unwrap();
        let req_cls = ffi::PyDict_GetItemString(worker_globals, req_class_name.as_ptr());

        let request_obj = if !req_cls.is_null() {
            ffi::PyObject_Call(req_cls, args, std::ptr::null_mut())
        } else {
            std::ptr::null_mut()
        };
        ffi::Py_DECREF(args);
        request_obj
    }

    /// Parse a Python result object into SubInterpResponse
    unsafe fn parse_result(
        &self,
        worker_idx: usize,
        result_obj: *mut ffi::PyObject,
    ) -> Result<SubInterpResponse, String> {
        if result_obj.is_null() {
            ffi::PyErr_Print();
            return Err("handler raised an exception".to_string());
        }

        // Check if it's a _SkyResponse
        let worker_globals = self.globals[worker_idx];
        let resp_class_name = std::ffi::CString::new("_SkyResponse").unwrap();
        let resp_cls = ffi::PyDict_GetItemString(worker_globals, resp_class_name.as_ptr());
        if !resp_cls.is_null() {
            let is_resp = ffi::PyObject_IsInstance(result_obj, resp_cls);
            if is_resp == 1 {
                return self.parse_sky_response(result_obj);
            }
        }

        // dict → JSON
        if ffi::PyDict_Check(result_obj) != 0 {
            let json_mod = ffi::PyImport_ImportModule(c"json".as_ptr());
            if json_mod.is_null() {
                ffi::Py_DECREF(result_obj);
                return Err("failed to import json".to_string());
            }
            let dumps = ffi::PyObject_GetAttrString(json_mod, c"dumps".as_ptr());
            let dump_args = ffi::PyTuple_New(1);
            ffi::Py_INCREF(result_obj);
            ffi::PyTuple_SetItem(dump_args, 0, result_obj);
            let json_str = ffi::PyObject_Call(dumps, dump_args, std::ptr::null_mut());
            ffi::Py_DECREF(dump_args);
            ffi::Py_DECREF(dumps);
            ffi::Py_DECREF(json_mod);
            ffi::Py_DECREF(result_obj);
            if json_str.is_null() {
                ffi::PyErr_Print();
                return Err("json.dumps failed".to_string());
            }
            let s = pyobj_to_string(json_str);
            ffi::Py_DECREF(json_str);
            return s.map(|body| SubInterpResponse {
                body,
                status: 200,
                content_type: None,
                headers: HashMap::new(),
                is_json: true,
            });
        }

        // string
        if ffi::PyUnicode_Check(result_obj) != 0 {
            let s = pyobj_to_string(result_obj);
            ffi::Py_DECREF(result_obj);
            return s.map(|body| SubInterpResponse {
                body,
                status: 200,
                content_type: None,
                headers: HashMap::new(),
                is_json: false,
            });
        }

        // fallback: str(result)
        let str_obj = ffi::PyObject_Str(result_obj);
        ffi::Py_DECREF(result_obj);
        if str_obj.is_null() {
            return Err("str() failed".to_string());
        }
        let s = pyobj_to_string(str_obj);
        ffi::Py_DECREF(str_obj);
        s.map(|body| SubInterpResponse {
            body,
            status: 200,
            content_type: None,
            headers: HashMap::new(),
            is_json: false,
        })
    }

    /// Parse a _SkyResponse Python object
    unsafe fn parse_sky_response(
        &self,
        obj: *mut ffi::PyObject,
    ) -> Result<SubInterpResponse, String> {
        // Extract status_code
        let status_attr = ffi::PyObject_GetAttrString(obj, c"status_code".as_ptr());
        let status = if !status_attr.is_null() {
            let s = ffi::PyLong_AsLong(status_attr) as u16;
            ffi::Py_DECREF(status_attr);
            s
        } else {
            ffi::PyErr_Clear();
            200
        };

        // Extract content_type
        let ct_attr = ffi::PyObject_GetAttrString(obj, c"content_type".as_ptr());
        let content_type = if !ct_attr.is_null() && ct_attr != ffi::Py_None() {
            let s = pyobj_to_string(ct_attr).ok();
            ffi::Py_DECREF(ct_attr);
            s
        } else {
            if !ct_attr.is_null() {
                ffi::Py_DECREF(ct_attr);
            }
            ffi::PyErr_Clear();
            None
        };

        // Extract headers dict
        let mut resp_headers = HashMap::new();
        let headers_attr = ffi::PyObject_GetAttrString(obj, c"headers".as_ptr());
        if !headers_attr.is_null() && ffi::PyDict_Check(headers_attr) != 0 {
            let mut pos: isize = 0;
            let mut key: *mut ffi::PyObject = std::ptr::null_mut();
            let mut val: *mut ffi::PyObject = std::ptr::null_mut();
            while ffi::PyDict_Next(headers_attr, &mut pos, &mut key, &mut val) != 0 {
                if let (Ok(k), Ok(v)) = (pyobj_to_string(key), pyobj_to_string(val)) {
                    resp_headers.insert(k, v);
                }
            }
            ffi::Py_DECREF(headers_attr);
        } else if !headers_attr.is_null() {
            ffi::Py_DECREF(headers_attr);
        }

        // Extract body
        let body_attr = ffi::PyObject_GetAttrString(obj, c"body".as_ptr());
        let (body, is_json) = if !body_attr.is_null() {
            if ffi::PyDict_Check(body_attr) != 0 {
                // Serialize dict to JSON
                let json_mod = ffi::PyImport_ImportModule(c"json".as_ptr());
                if !json_mod.is_null() {
                    let dumps = ffi::PyObject_GetAttrString(json_mod, c"dumps".as_ptr());
                    let args = ffi::PyTuple_New(1);
                    ffi::Py_INCREF(body_attr);
                    ffi::PyTuple_SetItem(args, 0, body_attr);
                    let json_str = ffi::PyObject_Call(dumps, args, std::ptr::null_mut());
                    ffi::Py_DECREF(args);
                    ffi::Py_DECREF(dumps);
                    ffi::Py_DECREF(json_mod);
                    if !json_str.is_null() {
                        let s = pyobj_to_string(json_str).unwrap_or_default();
                        ffi::Py_DECREF(json_str);
                        (s, true)
                    } else {
                        ffi::PyErr_Clear();
                        (String::new(), false)
                    }
                } else {
                    ffi::PyErr_Clear();
                    (String::new(), false)
                }
            } else if ffi::PyUnicode_Check(body_attr) != 0 {
                let s = pyobj_to_string(body_attr).unwrap_or_default();
                (s, false)
            } else {
                let str_obj = ffi::PyObject_Str(body_attr);
                let s = if !str_obj.is_null() {
                    let r = pyobj_to_string(str_obj).unwrap_or_default();
                    ffi::Py_DECREF(str_obj);
                    r
                } else {
                    ffi::PyErr_Clear();
                    String::new()
                };
                (s, false)
            }
        } else {
            ffi::PyErr_Clear();
            (String::new(), false)
        };
        if !body_attr.is_null() {
            ffi::Py_DECREF(body_attr);
        }

        ffi::Py_DECREF(obj);

        Ok(SubInterpResponse {
            body,
            status,
            content_type,
            headers: resp_headers,
            is_json,
        })
    }

    /// Call a handler in a sub-interpreter. Returns SubInterpResponse.
    unsafe fn call_handler(
        &self,
        worker_idx: usize,
        handler_idx: usize,
        method: &str,
        path: &str,
        params: &HashMap<String, String>,
        query: &str,
        body: &[u8],
        headers: &HashMap<String, String>,
    ) -> Result<SubInterpResponse, String> {
        let _guard = self.locks[worker_idx].lock();

        let hmap = &self.handler_maps[worker_idx];
        let handler_name = &self.handler_names[handler_idx];

        let func = match hmap.get(handler_name) {
            Some(f) => *f,
            None => {
                return Err(format!(
                    "handler '{}' not found in sub-interpreter",
                    handler_name
                ))
            }
        };

        // Acquire this sub-interpreter's GIL
        let worker = &self.workers[worker_idx];
        ffi::PyEval_RestoreThread(worker.tstate);

        // Build request object
        let request_obj =
            self.build_request_obj(worker_idx, method, path, params, query, body, headers);
        if request_obj.is_null() {
            ffi::PyErr_Print();
            let _saved = ffi::PyEval_SaveThread();
            return Err("failed to create request object".to_string());
        }

        // Run before_request hooks
        for hook_name in &self.before_hook_names {
            if let Some(&hook_func) = hmap.get(hook_name) {
                let hook_args = ffi::PyTuple_New(1);
                ffi::Py_INCREF(request_obj);
                ffi::PyTuple_SetItem(hook_args, 0, request_obj);
                let hook_result =
                    ffi::PyObject_Call(hook_func, hook_args, std::ptr::null_mut());
                ffi::Py_DECREF(hook_args);
                if !hook_result.is_null() && hook_result != ffi::Py_None() {
                    // Short-circuit: hook returned a response
                    ffi::Py_DECREF(request_obj);
                    let resp = self.parse_result(worker_idx, hook_result);
                    let _saved = ffi::PyEval_SaveThread();
                    return resp;
                }
                if !hook_result.is_null() {
                    ffi::Py_DECREF(hook_result);
                }
                if hook_result.is_null() {
                    ffi::PyErr_Print();
                }
            }
        }

        // Call handler(request)
        let call_args = ffi::PyTuple_New(1);
        ffi::PyTuple_SetItem(call_args, 0, request_obj); // steals ref

        let result_obj = ffi::PyObject_Call(func, call_args, std::ptr::null_mut());
        ffi::Py_DECREF(call_args);

        let result = self.parse_result(worker_idx, result_obj);

        // Release this sub-interpreter's GIL
        let _saved = ffi::PyEval_SaveThread();

        result
    }
}

/// Extract a Rust String from a Python str object (raw FFI)
unsafe fn pyobj_to_string(obj: *mut ffi::PyObject) -> Result<String, String> {
    let mut size: isize = 0;
    let ptr = ffi::PyUnicode_AsUTF8AndSize(obj, &mut size);
    if ptr.is_null() {
        return Err("failed to extract string".to_string());
    }
    let bytes = std::slice::from_raw_parts(ptr as *const u8, size as usize);
    String::from_utf8(bytes.to_vec()).map_err(|e| e.to_string())
}

type SharedPool = Arc<InterpreterPool>;

// ---------------------------------------------------------------------------
// Extract headers from hyper request
// ---------------------------------------------------------------------------

fn extract_headers(header_map: &hyper::HeaderMap) -> HashMap<String, String> {
    let mut headers = HashMap::with_capacity(header_map.len());
    for (name, value) in header_map.iter() {
        let key = name.as_str().to_string();
        let val = String::from_utf8_lossy(value.as_bytes()).to_string();
        // If key already exists, join with ", " per RFC 9110
        headers
            .entry(key)
            .and_modify(|existing: &mut String| {
                existing.push_str(", ");
                existing.push_str(&val);
            })
            .or_insert(val);
    }
    headers
}

// ---------------------------------------------------------------------------
// SkyApp
// ---------------------------------------------------------------------------

#[pyclass]
struct SkyApp {
    routes: SharedRoutes,
    script_path: Option<String>,
}

#[pymethods]
impl SkyApp {
    #[new]
    fn new() -> Self {
        SkyApp {
            routes: Arc::new(RwLock::new(RouteTable::new())),
            script_path: None,
        }
    }

    fn get(&mut self, path: &str, handler: Py<PyAny>, py: Python<'_>) -> PyResult<()> {
        let name = handler.getattr(py, "__name__")?.extract::<String>(py)?;
        self.add_route("GET", path, handler, name)
    }

    fn post(&mut self, path: &str, handler: Py<PyAny>, py: Python<'_>) -> PyResult<()> {
        let name = handler.getattr(py, "__name__")?.extract::<String>(py)?;
        self.add_route("POST", path, handler, name)
    }

    fn put(&mut self, path: &str, handler: Py<PyAny>, py: Python<'_>) -> PyResult<()> {
        let name = handler.getattr(py, "__name__")?.extract::<String>(py)?;
        self.add_route("PUT", path, handler, name)
    }

    fn delete(&mut self, path: &str, handler: Py<PyAny>, py: Python<'_>) -> PyResult<()> {
        let name = handler.getattr(py, "__name__")?.extract::<String>(py)?;
        self.add_route("DELETE", path, handler, name)
    }

    fn route(
        &mut self,
        method: &str,
        path: &str,
        handler: Py<PyAny>,
        py: Python<'_>,
    ) -> PyResult<()> {
        let name = handler.getattr(py, "__name__")?.extract::<String>(py)?;
        self.add_route(method, path, handler, name)
    }

    fn before_request(&mut self, handler: Py<PyAny>, py: Python<'_>) -> PyResult<()> {
        let name = handler.getattr(py, "__name__")?.extract::<String>(py)?;
        let mut routes = self.routes.write();
        routes.before_hooks.push(handler);
        routes.before_hook_names.push(name);
        Ok(())
    }

    fn after_request(&mut self, handler: Py<PyAny>, py: Python<'_>) -> PyResult<()> {
        let name = handler.getattr(py, "__name__")?.extract::<String>(py)?;
        let mut routes = self.routes.write();
        routes.after_hooks.push(handler);
        routes.after_hook_names.push(name);
        Ok(())
    }

    fn fallback(&mut self, handler: Py<PyAny>, py: Python<'_>) -> PyResult<()> {
        let name = handler.getattr(py, "__name__")?.extract::<String>(py)?;
        let mut routes = self.routes.write();
        routes.fallback_handler = Some(handler);
        routes.fallback_handler_name = Some(name);
        Ok(())
    }

    fn static_dir(&mut self, prefix: &str, directory: &str) -> PyResult<()> {
        let prefix = if prefix.ends_with('/') {
            prefix.to_string()
        } else {
            format!("{prefix}/")
        };
        let dir = std::path::Path::new(directory)
            .canonicalize()
            .map_err(|e| {
                pyo3::exceptions::PyValueError::new_err(format!(
                    "static directory '{directory}' not found: {e}"
                ))
            })?
            .to_string_lossy()
            .to_string();
        let mut routes = self.routes.write();
        routes.static_dirs.push((prefix, dir));
        Ok(())
    }

    #[pyo3(signature = (host=None, port=None, workers=None, mode=None))]
    fn run(
        &self,
        py: Python<'_>,
        host: Option<&str>,
        port: Option<u16>,
        workers: Option<usize>,
        mode: Option<&str>,
    ) -> PyResult<()> {
        let host = host.unwrap_or("127.0.0.1");
        let port = port.unwrap_or(8000);
        let mode = mode.unwrap_or("default");
        let addr: SocketAddr = format!("{host}:{port}")
            .parse()
            .map_err(|e: std::net::AddrParseError| {
                pyo3::exceptions::PyValueError::new_err(e.to_string())
            })?;

        let num_cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let workers = workers.unwrap_or(num_cpus);

        let routes = Arc::clone(&self.routes);

        if mode == "subinterp" {
            let script_path = if let Some(ref p) = self.script_path {
                p.clone()
            } else {
                let main_mod = py.import("__main__")?;
                main_mod.getattr("__file__")?.extract::<String>()?
            };

            let (handler_names, routers, before_hook_names, after_hook_names, static_dirs) = {
                let table = routes.read();
                (
                    table.handler_names.clone(),
                    table.routers.clone(),
                    table.before_hook_names.clone(),
                    table.after_hook_names.clone(),
                    table.static_dirs.clone(),
                )
            };

            println!("\n  Pyre v0.3.0 [sub-interpreter mode]");
            println!("  Listening on http://{addr}");
            println!("  Sub-interpreters: {workers} (CPUs: {num_cpus})");
            println!("  Script: {script_path}\n");

            let pool = unsafe {
                InterpreterPool::new(
                    workers,
                    &script_path,
                    &handler_names,
                    routers,
                    &before_hook_names,
                    &after_hook_names,
                    static_dirs,
                )
                .map_err(|e| {
                    pyo3::exceptions::PyRuntimeError::new_err(format!(
                        "sub-interpreter pool error: {e}"
                    ))
                })?
            };
            let pool = Arc::new(pool);

            py.detach(move || -> PyResult<()> {
                let rt = RuntimeBuilder::new_multi_thread()
                    .worker_threads(workers)
                    .enable_all()
                    .build()
                    .map_err(|e| {
                        pyo3::exceptions::PyRuntimeError::new_err(format!(
                            "tokio runtime error: {e}"
                        ))
                    })?;

                rt.block_on(async move {
                    let listener = TcpListener::bind(addr).await.map_err(|e| {
                        pyo3::exceptions::PyOSError::new_err(format!("bind error: {e}"))
                    })?;

                    let shutdown = async {
                        signal::ctrl_c().await.ok();
                        println!("\n  Shutting down gracefully...");
                    };
                    tokio::pin!(shutdown);

                    loop {
                        tokio::select! {
                            result = listener.accept() => {
                                let (stream, _) = result.map_err(|e| {
                                    pyo3::exceptions::PyOSError::new_err(format!("accept error: {e}"))
                                })?;

                                let pool = Arc::clone(&pool);
                                let io = TokioIo::new(stream);

                                tokio::spawn(async move {
                                    let svc = service_fn(move |req: Request<Incoming>| {
                                        let pool = Arc::clone(&pool);
                                        async move { handle_request_subinterp(req, pool).await }
                                    });

                                    if let Err(e) = http1::Builder::new()
                                        .keep_alive(true)
                                        .pipeline_flush(true)
                                        .serve_connection(io, svc)
                                        .await
                                    {
                                        let msg = e.to_string();
                                        if !msg.contains("connection closed")
                                            && !msg.contains("reset by peer")
                                            && !msg.contains("broken pipe")
                                        {
                                            eprintln!("connection error: {e}");
                                        }
                                    }
                                });
                            }
                            _ = &mut shutdown => {
                                break;
                            }
                        }
                    }

                    Ok(())
                })
            })
        } else {
            println!("\n  Pyre v0.3.0");
            println!("  Listening on http://{addr}");
            println!("  Workers: {workers} (CPUs: {num_cpus})\n");

            py.detach(move || -> PyResult<()> {
                let rt = RuntimeBuilder::new_multi_thread()
                    .worker_threads(workers)
                    .enable_all()
                    .build()
                    .map_err(|e| {
                        pyo3::exceptions::PyRuntimeError::new_err(format!(
                            "tokio runtime error: {e}"
                        ))
                    })?;

                rt.block_on(async move {
                    let listener = TcpListener::bind(addr).await.map_err(|e| {
                        pyo3::exceptions::PyOSError::new_err(format!("bind error: {e}"))
                    })?;

                    let shutdown = async {
                        signal::ctrl_c().await.ok();
                        println!("\n  Shutting down gracefully...");
                    };
                    tokio::pin!(shutdown);

                    loop {
                        tokio::select! {
                            result = listener.accept() => {
                                let (stream, _) = result.map_err(|e| {
                                    pyo3::exceptions::PyOSError::new_err(format!("accept error: {e}"))
                                })?;

                                let routes = Arc::clone(&routes);
                                let io = TokioIo::new(stream);

                                tokio::spawn(async move {
                                    let svc = service_fn(move |req: Request<Incoming>| {
                                        let routes = Arc::clone(&routes);
                                        async move { handle_request(req, routes).await }
                                    });

                                    if let Err(e) = http1::Builder::new()
                                        .keep_alive(true)
                                        .pipeline_flush(true)
                                        .serve_connection(io, svc)
                                        .await
                                    {
                                        let msg = e.to_string();
                                        if !msg.contains("connection closed")
                                            && !msg.contains("reset by peer")
                                            && !msg.contains("broken pipe")
                                        {
                                            eprintln!("connection error: {e}");
                                        }
                                    }
                                });
                            }
                            _ = &mut shutdown => {
                                break;
                            }
                        }
                    }

                    Ok(())
                })
            })
        }
    }
}

impl SkyApp {
    fn add_route(
        &mut self,
        method: &str,
        path: &str,
        handler: Py<PyAny>,
        handler_name: String,
    ) -> PyResult<()> {
        let mut routes = self.routes.write();
        routes
            .insert(method, path, handler, handler_name)
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(format!("route error: {e}")))?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Convert handler return value to ResponseData (main interpreter mode)
// ---------------------------------------------------------------------------

fn extract_response_data(py: Python<'_>, obj: Bound<'_, pyo3::PyAny>) -> Result<ResponseData, String> {
    // Check if it's a SkyResponse
    if let Ok(resp) = obj.downcast::<SkyResponse>() {
        let resp = resp.get();
        let body_bound = resp.body.bind(py);

        let (body_bytes, auto_ct) = if let Ok(s) = body_bound.downcast::<PyString>() {
            let st = s.to_string();
            let ct = if st.starts_with('{') || st.starts_with('[') {
                "application/json"
            } else {
                "text/plain; charset=utf-8"
            };
            (Bytes::from(st), ct)
        } else if body_bound.downcast::<PyDict>().is_ok() || body_bound.downcast::<PyList>().is_ok()
        {
            let val = py_to_json_value(body_bound).map_err(|e| format!("json error: {e}"))?;
            let json_bytes =
                serde_json::to_vec(&val).map_err(|e| format!("json serialize error: {e}"))?;
            (Bytes::from(json_bytes), "application/json")
        } else if let Ok(b) = body_bound.extract::<Vec<u8>>() {
            (Bytes::from(b), "application/octet-stream")
        } else {
            let st = body_bound.str().map_err(|e| e.to_string())?.to_string();
            (Bytes::from(st), "text/plain; charset=utf-8")
        };

        let content_type = resp
            .content_type
            .clone()
            .unwrap_or_else(|| auto_ct.to_string());

        return Ok(ResponseData {
            body: body_bytes,
            content_type,
            status: resp.status_code,
            headers: resp.headers.clone(),
        });
    }

    // Plain string
    if let Ok(s) = obj.downcast::<PyString>() {
        let st = s.to_string();
        let ct = if st.starts_with('{') || st.starts_with('[') {
            "application/json"
        } else {
            "text/plain; charset=utf-8"
        };
        return Ok(ResponseData {
            body: Bytes::from(st),
            content_type: ct.to_string(),
            status: 200,
            headers: HashMap::new(),
        });
    }

    // dict → JSON
    if obj.downcast::<PyDict>().is_ok() {
        let val = py_to_json_value(&obj).map_err(|e| format!("json error: {e}"))?;
        let json_bytes =
            serde_json::to_vec(&val).map_err(|e| format!("json serialize error: {e}"))?;
        return Ok(ResponseData {
            body: Bytes::from(json_bytes),
            content_type: "application/json".to_string(),
            status: 200,
            headers: HashMap::new(),
        });
    }

    // list → JSON
    if obj.downcast::<PyList>().is_ok() {
        let val = py_to_json_value(&obj).map_err(|e| format!("json error: {e}"))?;
        let json_bytes =
            serde_json::to_vec(&val).map_err(|e| format!("json serialize error: {e}"))?;
        return Ok(ResponseData {
            body: Bytes::from(json_bytes),
            content_type: "application/json".to_string(),
            status: 200,
            headers: HashMap::new(),
        });
    }

    // bytes
    if let Ok(b) = obj.extract::<Vec<u8>>() {
        return Ok(ResponseData {
            body: Bytes::from(b),
            content_type: "application/octet-stream".to_string(),
            status: 200,
            headers: HashMap::new(),
        });
    }

    // fallback: str()
    let st = obj.str().map_err(|e| e.to_string())?.to_string();
    Ok(ResponseData {
        body: Bytes::from(st),
        content_type: "text/plain; charset=utf-8".to_string(),
        status: 200,
        headers: HashMap::new(),
    })
}

// ---------------------------------------------------------------------------
// Request handler — default mode (main interpreter)
// ---------------------------------------------------------------------------

async fn handle_request(
    req: Request<Incoming>,
    routes: SharedRoutes,
) -> Result<Response<Full<Bytes>>, hyper::Error> {
    let method = req.method().to_string();
    let uri = req.uri().clone();
    let path = uri.path().to_string();
    let query = uri.query().unwrap_or("").to_string();
    let headers = extract_headers(req.headers());

    use http_body_util::BodyExt;
    let body_bytes = req
        .into_body()
        .collect()
        .await
        .map(|c| c.to_bytes().to_vec())
        .unwrap_or_default();

    let (lookup, static_dirs, has_fallback) = {
        let table = routes.read();
        (
            table.lookup(&method, &path),
            table.static_dirs.clone(),
            table.fallback_handler.is_some(),
        )
    };

    // If no route matched, try static files
    if lookup.is_none() {
        if let Some(resp) = try_static_file(&path, &static_dirs).await {
            return Ok(resp);
        }
    }

    let (handler_idx, params) = match lookup {
        Some(v) => v,
        None if has_fallback => {
            // Will use fallback handler below
            (usize::MAX, HashMap::new())
        }
        None => return Ok(not_found_response()),
    };

    let sky_req = SkyRequest {
        method,
        path,
        params,
        query,
        headers,
        body_bytes,
    };

    let result: Result<ResponseData, String> = Python::attach(|py| {
        let table = routes.read();
        let before_hooks: Vec<Py<PyAny>> = table.before_hooks.iter().map(|h| h.clone_ref(py)).collect();
        let after_hooks: Vec<Py<PyAny>> = table.after_hooks.iter().map(|h| h.clone_ref(py)).collect();

        let handler = if handler_idx == usize::MAX {
            // Fallback handler
            table.fallback_handler.as_ref().unwrap().clone_ref(py)
        } else {
            table.handlers[handler_idx].clone_ref(py)
        };
        drop(table);

        // Run before_request hooks
        for hook in &before_hooks {
            match hook.call1(py, (sky_req.clone(),)) {
                Ok(result) => {
                    let bound = result.bind(py);
                    if !bound.is_none() {
                        return extract_response_data(py, bound.clone());
                    }
                }
                Err(e) => return Err(format!("before_request hook error: {e}")),
            }
        }

        // Call main handler
        match handler.call1(py, (sky_req.clone(),)) {
            Ok(obj) => {
                let mut resp_data = extract_response_data(py, obj.bind(py).clone())?;

                // Run after_request hooks
                for hook in &after_hooks {
                    let body_str = std::str::from_utf8(&resp_data.body).unwrap_or("").to_string();
                    let current_resp = Py::new(py, SkyResponse {
                        body: PyString::new(py, &body_str)
                            .into_any()
                            .unbind(),
                        status_code: resp_data.status,
                        content_type: Some(resp_data.content_type.clone()),
                        headers: resp_data.headers.clone(),
                    }).map_err(|e| format!("failed to create SkyResponse: {e}"))?;
                    match hook.call1(py, (sky_req.clone(), current_resp)) {
                        Ok(result) => {
                            let bound = result.bind(py);
                            if !bound.is_none() {
                                resp_data = extract_response_data(py, bound.clone())?;
                            }
                        }
                        Err(e) => return Err(format!("after_request hook error: {e}")),
                    }
                }

                Ok(resp_data)
            }
            Err(e) => Err(format!("handler error: {e}")),
        }
    });

    build_response(result)
}

// ---------------------------------------------------------------------------
// Request handler — sub-interpreter mode
// ---------------------------------------------------------------------------

async fn handle_request_subinterp(
    req: Request<Incoming>,
    pool: SharedPool,
) -> Result<Response<Full<Bytes>>, hyper::Error> {
    let method = req.method().to_string();
    let uri = req.uri().clone();
    let path = uri.path().to_string();
    let query = uri.query().unwrap_or("").to_string();
    let headers = extract_headers(req.headers());

    use http_body_util::BodyExt;
    let body_bytes = req
        .into_body()
        .collect()
        .await
        .map(|c| c.to_bytes().to_vec())
        .unwrap_or_default();

    let lookup = pool.lookup(&method, &path);

    // Try static files if no route matched
    if lookup.is_none() {
        if let Some(resp) = try_static_file(&path, &pool.static_dirs).await {
            return Ok(resp);
        }
    }

    let (handler_idx, params) = match lookup {
        Some(v) => v,
        None => return Ok(not_found_response()),
    };

    let worker_idx = pool.pick_worker();

    let result = unsafe {
        pool.call_handler(
            worker_idx,
            handler_idx,
            &method,
            &path,
            &params,
            &query,
            &body_bytes,
            &headers,
        )
    };

    match result {
        Ok(resp) => {
            let ct = resp.content_type.unwrap_or_else(|| {
                if resp.is_json || resp.body.starts_with('{') || resp.body.starts_with('[') {
                    "application/json".to_string()
                } else {
                    "text/plain; charset=utf-8".to_string()
                }
            });
            let status = StatusCode::from_u16(resp.status).unwrap_or(StatusCode::OK);
            let mut builder = Response::builder()
                .status(status)
                .header("content-type", &ct)
                .header("server", "Pyre/0.3.0-subinterp");
            for (k, v) in &resp.headers {
                builder = builder.header(k.as_str(), v.as_str());
            }
            Ok(builder
                .body(Full::new(Bytes::from(resp.body)))
                .unwrap())
        }
        Err(e) => Ok(Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .header("content-type", "application/json")
            .header("server", "Pyre/0.3.0-subinterp")
            .body(Full::new(Bytes::from(
                format!(r#"{{"error":"{}"}}"#, e.replace('"', "\\\"")),
            )))
            .unwrap()),
    }
}

// ---------------------------------------------------------------------------
// Shared response builder
// ---------------------------------------------------------------------------

fn build_response(
    result: Result<ResponseData, String>,
) -> Result<Response<Full<Bytes>>, hyper::Error> {
    match result {
        Ok(data) => {
            let status = StatusCode::from_u16(data.status).unwrap_or(StatusCode::OK);
            let mut builder = Response::builder()
                .status(status)
                .header("content-type", &data.content_type)
                .header("server", "Pyre/0.3.0");
            for (k, v) in &data.headers {
                builder = builder.header(k.as_str(), v.as_str());
            }
            Ok(builder.body(Full::new(data.body)).unwrap())
        }
        Err(e) => Ok(Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .header("content-type", "application/json")
            .header("server", "Pyre/0.3.0")
            .body(Full::new(Bytes::from(
                format!(r#"{{"error":"{}"}}"#, e.replace('"', "\\\"")),
            )))
            .unwrap()),
    }
}

// ---------------------------------------------------------------------------
// Static file serving
// ---------------------------------------------------------------------------

fn mime_from_ext(ext: &str) -> &'static str {
    match ext {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "application/javascript; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "webp" => "image/webp",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "pdf" => "application/pdf",
        "xml" => "application/xml; charset=utf-8",
        "txt" => "text/plain; charset=utf-8",
        "wasm" => "application/wasm",
        "map" => "application/json",
        _ => "application/octet-stream",
    }
}

async fn try_static_file(
    req_path: &str,
    static_dirs: &[(String, String)],
) -> Option<Response<Full<Bytes>>> {
    for (prefix, directory) in static_dirs {
        if !req_path.starts_with(prefix.as_str()) {
            continue;
        }
        let rel = &req_path[prefix.len()..];
        // Reject path traversal
        if rel.contains("..") {
            return Some(
                Response::builder()
                    .status(StatusCode::FORBIDDEN)
                    .header("server", "Pyre/0.3.0")
                    .body(Full::new(Bytes::from_static(b"forbidden")))
                    .unwrap(),
            );
        }
        let file_path = std::path::PathBuf::from(directory).join(rel);
        if let Ok(contents) = tokio::fs::read(&file_path).await {
            let ext = file_path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            let ct = mime_from_ext(ext);
            return Some(
                Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", ct)
                    .header("server", "Pyre/0.3.0")
                    .body(Full::new(Bytes::from(contents)))
                    .unwrap(),
            );
        }
    }
    None
}

#[inline]
fn not_found_response() -> Response<Full<Bytes>> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header("content-type", "application/json")
        .header("server", "Pyre/0.3.0")
        .body(Full::new(Bytes::from_static(b"{\"error\":\"not found\"}")))
        .unwrap()
}

// ---------------------------------------------------------------------------
// Python module
// ---------------------------------------------------------------------------

#[pymodule]
fn engine(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<SkyApp>()?;
    m.add_class::<SkyRequest>()?;
    m.add_class::<SkyResponse>()?;
    Ok(())
}
