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
// Request object exposed to Python handlers (main interpreter only)
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

struct RouteEntry {
    handler_name: String, // function name for sub-interpreter mode
}

struct RouteTable {
    handlers: Vec<Py<PyAny>>,
    handler_names: Vec<String>,
    routers: HashMap<String, Router<usize>>,
}

impl RouteTable {
    fn new() -> Self {
        RouteTable {
            handlers: Vec::new(),
            handler_names: Vec::new(),
            routers: HashMap::new(),
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

    /// Get route info for sub-interpreter mode: Vec<(method, path, handler_name)>
    fn route_specs(&self) -> Vec<(String, String, String)> {
        let mut specs = Vec::new();
        for (method, router) in &self.routers {
            // We need to reconstruct path→handler_name mapping
            // matchit doesn't expose iteration, so we store separately
            for (i, name) in self.handler_names.iter().enumerate() {
                // Check if this handler index belongs to this method's router
                // by checking all routers
                specs.push((method.clone(), String::new(), name.clone()));
            }
        }
        specs
    }
}

unsafe impl Send for RouteTable {}
unsafe impl Sync for RouteTable {}

type SharedRoutes = Arc<RwLock<RouteTable>>;

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
    /// Per-worker mutex to ensure only one thread uses a sub-interpreter at a time
    locks: Vec<parking_lot::Mutex<()>>,
    counter: AtomicUsize,
    /// For each worker: a mapping of handler_name → PyObject pointer
    handler_maps: Vec<HashMap<String, *mut ffi::PyObject>>,
    /// For each worker: the globals dict (owns _SkyRequest class etc.)
    globals: Vec<*mut ffi::PyObject>,
    /// The route table for lookups (method, path) → (handler_idx, params)
    routers: HashMap<String, Router<usize>>,
    handler_names: Vec<String>,
}

unsafe impl Send for InterpreterPool {}
unsafe impl Sync for InterpreterPool {}

impl InterpreterPool {
    /// Create N sub-interpreters, each loading the given Python script
    unsafe fn new(
        n: usize,
        script_path: &str,
        handler_names: &[String],
        routers: HashMap<String, Router<usize>>,
    ) -> Result<Self, String> {
        let mut workers = Vec::with_capacity(n);
        let mut handler_maps = Vec::with_capacity(n);
        let mut all_globals = Vec::with_capacity(n);

        // Save the current thread state (main interpreter)
        let main_tstate = ffi::PyThreadState_Get();

        for i in 0..n {
            // Create a new sub-interpreter with its own GIL
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

            let status =
                ffi::Py_NewInterpreterFromConfig(&mut new_tstate, &config);
            if ffi::PyStatus_IsError(status) != 0 || new_tstate.is_null() {
                // Switch back to main
                ffi::PyThreadState_Swap(main_tstate);
                return Err(format!("Failed to create sub-interpreter {i}"));
            }

            // We are now in the sub-interpreter's thread state
            // Load the user script to get handler functions
            let mut hmap = HashMap::new();

            // Read the script file and filter out framework-specific lines
            let raw_script = std::fs::read_to_string(script_path)
                .map_err(|e| format!("Failed to read script: {e}"))?;

            // Filter out lines that import/use the framework or call app methods
            // Keep only pure Python function definitions and their dependencies
            let script: String = raw_script
                .lines()
                .filter(|line| {
                    let trimmed = line.trim();
                    // Skip framework imports
                    if trimmed.starts_with("from skytrade") || trimmed.starts_with("import skytrade") {
                        return false;
                    }
                    // Skip app = SkyApp() and app.get/post/put/delete/route/run calls
                    if trimmed.starts_with("app = ") || trimmed.starts_with("app.") {
                        return false;
                    }
                    // Skip if __name__ == "__main__" block
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
    def __init__(self, method, path, params, query, body_bytes):
        self.method = method
        self.path = path
        self.params = params
        self.query = query
        self.body_bytes = body_bytes
    @property
    def body(self):
        return self.body_bytes
    def text(self):
        return self.body_bytes.decode('utf-8') if isinstance(self.body_bytes, bytes) else str(self.body_bytes)
    def json(self):
        import json
        return json.loads(self.text())

# Execute user script
{}
"#,
                script
            );

            // Run the bootstrap code
            let globals = ffi::PyDict_New();
            let builtins = ffi::PyEval_GetBuiltins();
            ffi::PyDict_SetItemString(globals, c"__builtins__".as_ptr(), builtins);

            let code_cstr = std::ffi::CString::new(bootstrap.as_bytes())
                .map_err(|e| format!("CString error: {e}"))?;
            let filename_cstr = std::ffi::CString::new(script_path)
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

            // Extract handler functions by name
            for name in handler_names {
                let name_cstr = std::ffi::CString::new(name.as_bytes())
                    .map_err(|e| format!("CString error: {e}"))?;
                let func = ffi::PyDict_GetItemString(globals, name_cstr.as_ptr());
                if !func.is_null() && ffi::PyCallable_Check(func) != 0 {
                    ffi::Py_INCREF(func);
                    hmap.insert(name.clone(), func);
                }
            }

            // Keep globals alive (the handler functions reference it)
            ffi::Py_INCREF(globals);

            // Release this sub-interpreter's GIL
            let saved = ffi::PyEval_SaveThread();

            workers.push(SubInterpreter { tstate: saved });
            handler_maps.push(hmap);
            all_globals.push(globals);
        }

        // Switch back to main interpreter
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

    /// Call a handler in a sub-interpreter. Returns response body string.
    unsafe fn call_handler(
        &self,
        worker_idx: usize,
        handler_idx: usize,
        method: &str,
        path: &str,
        params: &HashMap<String, String>,
        query: &str,
        body: &[u8],
    ) -> Result<String, String> {
        // Lock this sub-interpreter so only one thread uses it at a time
        let _guard = self.locks[worker_idx].lock();

        let worker = &self.workers[worker_idx];
        let hmap = &self.handler_maps[worker_idx];
        let handler_name = &self.handler_names[handler_idx];

        let func = match hmap.get(handler_name) {
            Some(f) => *f,
            None => return Err(format!("handler '{}' not found in sub-interpreter", handler_name)),
        };

        // Acquire this sub-interpreter's GIL
        ffi::PyEval_RestoreThread(worker.tstate);

        // Build a _SkyRequest object
        let req_class_name = std::ffi::CString::new("_SkyRequest").unwrap();

        // Build args as Python objects
        let py_method = ffi::PyUnicode_FromStringAndSize(
            method.as_ptr() as *const _,
            method.len() as isize,
        );
        let py_path = ffi::PyUnicode_FromStringAndSize(
            path.as_ptr() as *const _,
            path.len() as isize,
        );

        // Build params dict
        let py_params = ffi::PyDict_New();
        for (k, v) in params {
            let pk = ffi::PyUnicode_FromStringAndSize(
                k.as_ptr() as *const _,
                k.len() as isize,
            );
            let pv = ffi::PyUnicode_FromStringAndSize(
                v.as_ptr() as *const _,
                v.len() as isize,
            );
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

        // Call _SkyRequest(method, path, params, query, body_bytes)
        // Get the class from builtins/globals — it's in the sub-interpreter's globals
        // Actually we need to call the handler directly with a request-like object
        // Let's build a SimpleNamespace-like dict instead for simplicity
        let req_dict = ffi::PyDict_New();
        let key_method = std::ffi::CString::new("method").unwrap();
        let key_path = std::ffi::CString::new("path").unwrap();
        let key_params = std::ffi::CString::new("params").unwrap();
        let key_query = std::ffi::CString::new("query").unwrap();
        let key_body = std::ffi::CString::new("body_bytes").unwrap();
        ffi::PyDict_SetItemString(req_dict, key_method.as_ptr(), py_method);
        ffi::PyDict_SetItemString(req_dict, key_path.as_ptr(), py_path);
        ffi::PyDict_SetItemString(req_dict, key_params.as_ptr(), py_params);
        ffi::PyDict_SetItemString(req_dict, key_query.as_ptr(), py_query);
        ffi::PyDict_SetItemString(req_dict, key_body.as_ptr(), py_body);

        // Create _SkyRequest via the globals
        // We bootstrapped _SkyRequest class in the sub-interpreter
        let args = ffi::PyTuple_New(5);
        ffi::PyTuple_SetItem(args, 0, py_method); // steals ref
        ffi::PyTuple_SetItem(args, 1, py_path);
        ffi::PyTuple_SetItem(args, 2, py_params);
        ffi::PyTuple_SetItem(args, 3, py_query);
        ffi::PyTuple_SetItem(args, 4, py_body);

        // Get _SkyRequest class from the sub-interpreter's globals
        let worker_globals = self.globals[worker_idx];
        let req_cls = ffi::PyDict_GetItemString(worker_globals, req_class_name.as_ptr());

        let request_obj = if !req_cls.is_null() {
            ffi::PyObject_Call(req_cls, args, std::ptr::null_mut())
        } else {
            // Fallback: just pass the dict
            ffi::Py_INCREF(req_dict);
            req_dict
        };
        ffi::Py_DECREF(args);

        if request_obj.is_null() {
            ffi::PyErr_Print();
            ffi::Py_DECREF(req_dict);
            let saved = ffi::PyEval_SaveThread();
            // Update tstate
            let worker = &self.workers[worker_idx];
            return Err("failed to create request object".to_string());
        }

        // Call handler(request)
        let call_args = ffi::PyTuple_New(1);
        ffi::PyTuple_SetItem(call_args, 0, request_obj); // steals ref

        let result_obj = ffi::PyObject_Call(func, call_args, std::ptr::null_mut());
        ffi::Py_DECREF(call_args);
        ffi::Py_DECREF(req_dict);

        let result = if result_obj.is_null() {
            ffi::PyErr_Print();
            Err("handler raised an exception".to_string())
        } else if ffi::PyDict_Check(result_obj) != 0 {
            // Dict → JSON serialize via Python json.dumps (in sub-interpreter)
            let json_mod = ffi::PyImport_ImportModule(c"json".as_ptr());
            if json_mod.is_null() {
                ffi::Py_DECREF(result_obj);
                Err("failed to import json".to_string())
            } else {
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
                    Err("json.dumps failed".to_string())
                } else {
                    let s = pyobj_to_string(json_str);
                    ffi::Py_DECREF(json_str);
                    s
                }
            }
        } else if ffi::PyUnicode_Check(result_obj) != 0 {
            let s = pyobj_to_string(result_obj);
            ffi::Py_DECREF(result_obj);
            s
        } else {
            // Fallback: str(result)
            let str_obj = ffi::PyObject_Str(result_obj);
            ffi::Py_DECREF(result_obj);
            if str_obj.is_null() {
                Err("str() failed".to_string())
            } else {
                let s = pyobj_to_string(str_obj);
                ffi::Py_DECREF(str_obj);
                s
            }
        };

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

    fn route(&mut self, method: &str, path: &str, handler: Py<PyAny>, py: Python<'_>) -> PyResult<()> {
        let name = handler.getattr(py, "__name__")?.extract::<String>(py)?;
        self.add_route(method, path, handler, name)
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
            // Detect script path from __main__.__file__
            let script_path = if let Some(ref p) = self.script_path {
                p.clone()
            } else {
                let main_mod = py.import("__main__")?;
                main_mod
                    .getattr("__file__")?
                    .extract::<String>()?
            };

            // Collect handler names and clone routers
            let (handler_names, routers) = {
                let table = routes.read();
                (table.handler_names.clone(), table.routers.clone())
            };

            println!("\n  Pyre v0.2.0 [sub-interpreter mode]");
            println!("  Listening on http://{addr}");
            println!("  Sub-interpreters: {workers} (CPUs: {num_cpus})");
            println!("  Script: {script_path}\n");

            // Create interpreter pool while we still hold the main GIL
            let pool = unsafe {
                InterpreterPool::new(workers, &script_path, &handler_names, routers)
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
            // Default mode: use main interpreter
            println!("\n  Pyre v0.2.0");
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
// Request handler — default mode (main interpreter)
// ---------------------------------------------------------------------------

async fn handle_request(
    req: Request<Incoming>,
    routes: SharedRoutes,
) -> Result<Response<Full<Bytes>>, hyper::Error> {
    let method = req.method().to_string();
    let path = req.uri().path().to_string();
    let query = req.uri().query().unwrap_or("").to_string();

    use http_body_util::BodyExt;
    let body_bytes = req
        .into_body()
        .collect()
        .await
        .map(|c| c.to_bytes().to_vec())
        .unwrap_or_default();

    let lookup = {
        let table = routes.read();
        table.lookup(&method, &path)
    };

    let (handler_idx, params) = match lookup {
        Some(v) => v,
        None => return Ok(not_found_response()),
    };

    let sky_req = SkyRequest {
        method,
        path,
        params,
        query,
        body_bytes,
    };

    let result: Result<(Bytes, &'static str), String> = Python::attach(|py| {
        let table = routes.read();
        let handler = table.handlers[handler_idx].clone_ref(py);
        drop(table);

        match handler.call1(py, (sky_req,)) {
            Ok(obj) => {
                let bound = obj.bind(py);

                if let Ok(s) = bound.downcast::<PyString>() {
                    let st = s.to_string();
                    let ct = if st.starts_with('{') || st.starts_with('[') {
                        "application/json"
                    } else {
                        "text/plain; charset=utf-8"
                    };
                    return Ok((Bytes::from(st), ct));
                }

                if bound.downcast::<PyDict>().is_ok() {
                    let val = py_to_json_value(bound).map_err(|e| format!("json error: {e}"))?;
                    let json_bytes = serde_json::to_vec(&val)
                        .map_err(|e| format!("json serialize error: {e}"))?;
                    return Ok((Bytes::from(json_bytes), "application/json"));
                }

                if bound.downcast::<PyList>().is_ok() {
                    let val = py_to_json_value(bound).map_err(|e| format!("json error: {e}"))?;
                    let json_bytes = serde_json::to_vec(&val)
                        .map_err(|e| format!("json serialize error: {e}"))?;
                    return Ok((Bytes::from(json_bytes), "application/json"));
                }

                if let Ok(b) = bound.extract::<Vec<u8>>() {
                    return Ok((Bytes::from(b), "application/octet-stream"));
                }

                let st = bound.str().map_err(|e| e.to_string())?.to_string();
                Ok((Bytes::from(st), "text/plain; charset=utf-8"))
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
    let path = req.uri().path().to_string();
    let query = req.uri().query().unwrap_or("").to_string();

    use http_body_util::BodyExt;
    let body_bytes = req
        .into_body()
        .collect()
        .await
        .map(|c| c.to_bytes().to_vec())
        .unwrap_or_default();

    let lookup = pool.lookup(&method, &path);

    let (handler_idx, params) = match lookup {
        Some(v) => v,
        None => return Ok(not_found_response()),
    };

    let worker_idx = pool.pick_worker();

    // Call handler in sub-interpreter (blocking — runs on Tokio thread)
    let result = unsafe {
        pool.call_handler(worker_idx, handler_idx, &method, &path, &params, &query, &body_bytes)
    };

    match result {
        Ok(body) => {
            let ct = if body.starts_with('{') || body.starts_with('[') {
                "application/json"
            } else {
                "text/plain; charset=utf-8"
            };
            Ok(Response::builder()
                .status(StatusCode::OK)
                .header("content-type", ct)
                .header("server", "Pyre/0.2.0-subinterp")
                .body(Full::new(Bytes::from(body)))
                .unwrap())
        }
        Err(e) => Ok(Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .header("content-type", "application/json")
            .header("server", "Pyre/0.2.0-subinterp")
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
    result: Result<(Bytes, &'static str), String>,
) -> Result<Response<Full<Bytes>>, hyper::Error> {
    match result {
        Ok((body, content_type)) => Ok(Response::builder()
            .status(StatusCode::OK)
            .header("content-type", content_type)
            .header("server", "Pyre/0.2.0")
            .body(Full::new(body))
            .unwrap()),
        Err(e) => Ok(Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .header("content-type", "application/json")
            .header("server", "Pyre/0.2.0")
            .body(Full::new(Bytes::from(
                format!(r#"{{"error":"{}"}}"#, e.replace('"', "\\\"")),
            )))
            .unwrap()),
    }
}

#[inline]
fn not_found_response() -> Response<Full<Bytes>> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .header("content-type", "application/json")
        .header("server", "Pyre/0.2.0")
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
    Ok(())
}
