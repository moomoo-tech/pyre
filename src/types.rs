use std::collections::HashMap;

use bytes::Bytes;
use pyo3::prelude::*;

// ---------------------------------------------------------------------------
// SkyRequest
// ---------------------------------------------------------------------------

#[pyclass(frozen)]
#[derive(Clone)]
pub(crate) struct SkyRequest {
    #[pyo3(get)]
    pub(crate) method: String,
    #[pyo3(get)]
    pub(crate) path: String,
    #[pyo3(get)]
    pub(crate) params: HashMap<String, String>,
    #[pyo3(get)]
    pub(crate) query: String,
    #[pyo3(get)]
    pub(crate) headers: HashMap<String, String>,
    pub(crate) body_bytes: Vec<u8>,
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
// SkyResponse
// ---------------------------------------------------------------------------

#[pyclass(frozen)]
pub(crate) struct SkyResponse {
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

pub(crate) fn extract_headers(header_map: &hyper::HeaderMap) -> HashMap<String, String> {
    let mut headers = HashMap::with_capacity(header_map.len());
    for (name, value) in header_map.iter() {
        let key = name.as_str().to_string();
        let val = String::from_utf8_lossy(value.as_bytes()).to_string();
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
