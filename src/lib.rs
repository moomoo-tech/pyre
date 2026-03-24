use std::collections::HashMap;
use std::net::SocketAddr;
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
use pyo3::prelude::*;
use pyo3::types::PyDict;
use tokio::net::TcpListener;
use tokio::runtime::Runtime;

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
// Route table: method -> matchit::Router<index>
// We store handlers in a Vec and use indices in the router to avoid Clone issues.
// ---------------------------------------------------------------------------

struct RouteTable {
    handlers: Vec<Py<PyAny>>,
    routers: HashMap<String, Router<usize>>,
}

impl RouteTable {
    fn new() -> Self {
        RouteTable {
            handlers: Vec::new(),
            routers: HashMap::new(),
        }
    }

    fn insert(&mut self, method: &str, path: &str, handler: Py<PyAny>) -> Result<(), String> {
        let idx = self.handlers.len();
        self.handlers.push(handler);
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

// Safety: Py<PyAny> is Send+Sync when GIL is not held
unsafe impl Send for RouteTable {}
unsafe impl Sync for RouteTable {}

type SharedRoutes = Arc<RwLock<RouteTable>>;

#[pyclass]
struct SkyApp {
    routes: SharedRoutes,
}

#[pymethods]
impl SkyApp {
    #[new]
    fn new() -> Self {
        SkyApp {
            routes: Arc::new(RwLock::new(RouteTable::new())),
        }
    }

    // -- route registration --------------------------------------------------

    fn get(&mut self, path: &str, handler: Py<PyAny>) -> PyResult<()> {
        self.add_route("GET", path, handler)
    }

    fn post(&mut self, path: &str, handler: Py<PyAny>) -> PyResult<()> {
        self.add_route("POST", path, handler)
    }

    fn put(&mut self, path: &str, handler: Py<PyAny>) -> PyResult<()> {
        self.add_route("PUT", path, handler)
    }

    fn delete(&mut self, path: &str, handler: Py<PyAny>) -> PyResult<()> {
        self.add_route("DELETE", path, handler)
    }

    fn route(&mut self, method: &str, path: &str, handler: Py<PyAny>) -> PyResult<()> {
        self.add_route(method, path, handler)
    }

    // -- server start --------------------------------------------------------

    fn run(&self, py: Python<'_>, host: Option<&str>, port: Option<u16>) -> PyResult<()> {
        let host = host.unwrap_or("127.0.0.1");
        let port = port.unwrap_or(8000);
        let addr: SocketAddr = format!("{host}:{port}")
            .parse()
            .map_err(|e: std::net::AddrParseError| {
                pyo3::exceptions::PyValueError::new_err(e.to_string())
            })?;

        let routes = Arc::clone(&self.routes);

        println!("\n  SkyTrade Engine v0.1.0");
        println!("  Listening on http://{addr}\n");

        // Release the GIL while the Tokio runtime runs the server.
        py.detach(move || -> PyResult<()> {
            let rt = Runtime::new().map_err(|e| {
                pyo3::exceptions::PyRuntimeError::new_err(format!("tokio runtime error: {e}"))
            })?;

            rt.block_on(async move {
                let listener = TcpListener::bind(addr).await.map_err(|e| {
                    pyo3::exceptions::PyOSError::new_err(format!("bind error: {e}"))
                })?;

                loop {
                    let (stream, _remote) = listener.accept().await.map_err(|e| {
                        pyo3::exceptions::PyOSError::new_err(format!("accept error: {e}"))
                    })?;

                    let routes = Arc::clone(&routes);
                    let io = TokioIo::new(stream);

                    tokio::spawn(async move {
                        let svc = service_fn(move |req: Request<Incoming>| {
                            let routes = Arc::clone(&routes);
                            async move { handle_request(req, routes).await }
                        });

                        if let Err(e) = http1::Builder::new().serve_connection(io, svc).await {
                            eprintln!("connection error: {e}");
                        }
                    });
                }
            })
        })
    }
}

// -- internal helpers --------------------------------------------------------

impl SkyApp {
    fn add_route(&mut self, method: &str, path: &str, handler: Py<PyAny>) -> PyResult<()> {
        let mut routes = self.routes.write();
        routes.insert(method, path, handler).map_err(|e| {
            pyo3::exceptions::PyValueError::new_err(format!("route error: {e}"))
        })?;
        Ok(())
    }
}

async fn handle_request(
    req: Request<Incoming>,
    routes: SharedRoutes,
) -> Result<Response<Full<Bytes>>, hyper::Error> {
    let method = req.method().to_string();
    let path = req.uri().path().to_string();
    let query = req.uri().query().unwrap_or("").to_string();

    // Collect request body bytes
    use http_body_util::BodyExt;
    let body_bytes = req
        .into_body()
        .collect()
        .await
        .map(|c| c.to_bytes().to_vec())
        .unwrap_or_default();

    // Route matching — get handler index and params
    let lookup = {
        let table = routes.read();
        table.lookup(&method, &path)
    };

    let (handler_idx, params) = match lookup {
        Some(v) => v,
        None => {
            return Ok(Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Full::new(Bytes::from(r#"{"error":"not found"}"#)))
                .unwrap());
        }
    };

    // Build SkyRequest
    let sky_req = SkyRequest {
        method,
        path,
        params,
        query,
        body_bytes,
    };

    // Acquire GIL and call the Python handler
    let result: Result<String, String> = Python::attach(|py| {
        // Clone the handler ref while holding GIL
        let table = routes.read();
        let handler = table.handlers[handler_idx].clone_ref(py);
        drop(table);

        let args = (sky_req,);
        match handler.call1(py, args) {
            Ok(obj) => {
                // If the handler returns a string, use it directly
                if let Ok(s) = obj.extract::<String>(py) {
                    return Ok(s);
                }
                // If the handler returns a dict, serialize it as JSON
                if obj.bind(py).cast::<PyDict>().is_ok() {
                    let json_mod = py.import("json").map_err(|e: PyErr| e.to_string())?;
                    let dumped = json_mod
                        .call_method1("dumps", (&obj,))
                        .map_err(|e: PyErr| e.to_string())?;
                    return dumped.extract::<String>().map_err(|e: PyErr| e.to_string());
                }
                // Fallback: str()
                Ok(obj.to_string())
            }
            Err(e) => Err(format!("handler error: {e}")),
        }
    });

    match result {
        Ok(body) => {
            let content_type = if body.starts_with('{') || body.starts_with('[') {
                "application/json"
            } else {
                "text/plain; charset=utf-8"
            };
            Ok(Response::builder()
                .status(StatusCode::OK)
                .header("content-type", content_type)
                .header("server", "SkyTrade-Engine/0.1.0")
                .body(Full::new(Bytes::from(body)))
                .unwrap())
        }
        Err(e) => Ok(Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(Full::new(Bytes::from(format!(r#"{{"error":"{e}"}}"#))))
            .unwrap()),
    }
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
