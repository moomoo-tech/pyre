use std::collections::HashMap;
use std::net::IpAddr;

use bytes::Bytes;
use pyo3::prelude::*;

// ---------------------------------------------------------------------------
// PyreRequest
// ---------------------------------------------------------------------------

#[pyclass(frozen, skip_from_py_object)]
#[derive(Clone)]
pub(crate) struct PyreRequest {
    #[pyo3(get)]
    pub(crate) method: String,
    #[pyo3(get)]
    pub(crate) path: String,
    /// Stored as Vec for small-count path params (typically 1-2).
    /// Exposed to Python as dict via custom getter.
    pub(crate) params: Vec<(String, String)>,
    #[pyo3(get)]
    pub(crate) query: String,
    #[pyo3(get)]
    pub(crate) headers: HashMap<String, String>,
    /// Raw IP — zero allocation. `.to_string()` only when Python accesses it.
    pub(crate) client_ip_addr: IpAddr,
    /// Stored as Bytes (ref-counted, zero-copy from hyper).
    pub(crate) body_bytes: Bytes,
}

#[pymethods]
impl PyreRequest {
    /// Converts Vec<(String, String)> → Python dict on access.
    #[getter]
    fn params(&self) -> HashMap<String, String> {
        self.params.iter().cloned().collect()
    }

    /// Lazy: heap-allocates the IP string only when Python reads `req.client_ip`.
    #[getter]
    fn client_ip(&self) -> String {
        self.client_ip_addr.to_string()
    }

    #[getter]
    fn body(&self) -> &[u8] {
        &self.body_bytes
    }

    /// Zero-copy: validates UTF-8 on the Bytes slice, creates Python str directly.
    fn text<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, pyo3::types::PyString>> {
        let s = std::str::from_utf8(&self.body_bytes)
            .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
        Ok(pyo3::types::PyString::new(py, s))
    }

    /// Feed raw bytes to json.loads — avoids Rust String allocation entirely.
    fn json<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, pyo3::PyAny>> {
        let json_mod = py.import("json")?;
        let py_bytes = pyo3::types::PyBytes::new(py, &self.body_bytes);
        json_mod.call_method1("loads", (py_bytes,))
    }

    #[getter]
    fn query_params(&self) -> HashMap<String, String> {
        form_urlencoded::parse(self.query.as_bytes())
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// PyreResponse
// ---------------------------------------------------------------------------

#[pyclass(frozen)]
pub(crate) struct PyreResponse {
    #[pyo3(get)]
    pub(crate) body: Py<PyAny>,
    #[pyo3(get)]
    pub(crate) status_code: u16,
    #[pyo3(get)]
    pub(crate) content_type: Option<String>,
    #[pyo3(get)]
    pub(crate) headers: HashMap<String, String>,
}

#[pymethods]
impl PyreResponse {
    #[new]
    #[pyo3(signature = (body, status_code=200, content_type=None, headers=None))]
    fn new(
        body: Py<PyAny>,
        status_code: u16,
        content_type: Option<String>,
        headers: Option<HashMap<String, String>>,
    ) -> Self {
        PyreResponse {
            body,
            status_code,
            content_type,
            headers: headers.unwrap_or_default(),
        }
    }
}

// ---------------------------------------------------------------------------
// ResponseData (Rust-internal, not exposed to Python)
// ---------------------------------------------------------------------------

pub(crate) struct ResponseData {
    pub(crate) body: Bytes,
    pub(crate) content_type: String,
    pub(crate) status: u16,
    pub(crate) headers: HashMap<String, String>,
}

// ---------------------------------------------------------------------------
// extract_headers
// ---------------------------------------------------------------------------

pub fn extract_headers(header_map: &hyper::HeaderMap) -> HashMap<String, String> {
    let mut headers = HashMap::with_capacity(header_map.len());
    for (name, value) in header_map.iter() {
        let key = name.as_str().to_string();
        // Fast path: valid UTF-8 (99.99% of headers) avoids Cow→String deep copy.
        let val = match std::str::from_utf8(value.as_bytes()) {
            Ok(s) => s.to_string(),
            Err(_) => String::from_utf8_lossy(value.as_bytes()).into_owned(),
        };
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_headers_basic() {
        let mut hm = hyper::HeaderMap::new();
        hm.insert("content-type", "application/json".parse().unwrap());
        hm.insert("x-custom", "hello".parse().unwrap());
        let h = extract_headers(&hm);
        assert_eq!(h["content-type"], "application/json");
        assert_eq!(h["x-custom"], "hello");
    }

    #[test]
    fn extract_headers_empty() {
        let hm = hyper::HeaderMap::new();
        let h = extract_headers(&hm);
        assert!(h.is_empty());
    }

    #[test]
    fn extract_headers_multi_value() {
        let mut hm = hyper::HeaderMap::new();
        hm.append("accept", "text/html".parse().unwrap());
        hm.append("accept", "application/json".parse().unwrap());
        let h = extract_headers(&hm);
        assert!(h["accept"].contains("text/html"));
        assert!(h["accept"].contains("application/json"));
        assert!(h["accept"].contains(", "));
    }

    #[test]
    fn query_params_parsing() {
        let req = PyreRequest {
            method: "GET".to_string(),
            path: "/search".to_string(),
            params: Vec::new(),
            query: "q=hello+world&page=2&lang=en".to_string(),
            headers: HashMap::new(),
            client_ip_addr: IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
            body_bytes: Bytes::new(),
        };
        let qp = req.query_params();
        assert_eq!(qp["q"], "hello world");
        assert_eq!(qp["page"], "2");
        assert_eq!(qp["lang"], "en");
    }

    #[test]
    fn query_params_empty() {
        let req = PyreRequest {
            method: "GET".to_string(),
            path: "/".to_string(),
            params: Vec::new(),
            query: "".to_string(),
            headers: HashMap::new(),
            client_ip_addr: IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
            body_bytes: Bytes::new(),
        };
        assert!(req.query_params().is_empty());
    }

    #[test]
    fn query_params_percent_encoded() {
        let req = PyreRequest {
            method: "GET".to_string(),
            path: "/".to_string(),
            params: Vec::new(),
            query: "name=%E4%B8%AD%E6%96%87".to_string(),
            headers: HashMap::new(),
            client_ip_addr: IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
            body_bytes: Bytes::new(),
        };
        assert_eq!(req.query_params()["name"], "中文");
    }

    // Note: text() and json() require Python GIL, tested via Python tests.
}
