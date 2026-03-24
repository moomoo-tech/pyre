//! SharedState — cross-sub-interpreter state sharing via DashMap.
//!
//! All sub-interpreters share the same Arc<DashMap> in Rust memory.
//! Python code uses `app.state["key"] = value` / `app.state["key"]`.
//! Values are stored as Rust Strings (serialize JSON at the boundary).

use std::sync::Arc;

use dashmap::DashMap;
use pyo3::prelude::*;

/// High-concurrency shared key-value store backed by DashMap.
///
/// Thread-safe, lock-free reads for different keys, nanosecond latency.
/// All sub-interpreters share the same underlying DashMap via Arc.
#[pyclass]
pub(crate) struct SharedState {
    inner: Arc<DashMap<String, Vec<u8>>>,
}

impl SharedState {
    /// Create a new SharedState with the given Arc (for sharing across workers).
    pub fn with_inner(inner: Arc<DashMap<String, Vec<u8>>>) -> Self {
        SharedState { inner }
    }

    /// Get the inner Arc for cloning into sub-interpreters.
    pub fn inner(&self) -> &Arc<DashMap<String, Vec<u8>>> {
        &self.inner
    }
}

#[pymethods]
impl SharedState {
    #[new]
    fn new() -> Self {
        SharedState {
            inner: Arc::new(DashMap::new()),
        }
    }

    /// Set a string value.
    fn set(&self, key: String, value: String) {
        self.inner.insert(key, value.into_bytes());
    }

    /// Get a string value. Returns None if key doesn't exist.
    fn get(&self, key: &str) -> Option<String> {
        self.inner
            .get(key)
            .and_then(|v| String::from_utf8(v.value().clone()).ok())
    }

    /// Set raw bytes value.
    fn set_bytes(&self, key: String, value: Vec<u8>) {
        self.inner.insert(key, value);
    }

    /// Get raw bytes value.
    fn get_bytes(&self, key: &str) -> Option<Vec<u8>> {
        self.inner.get(key).map(|v| v.value().clone())
    }

    /// Delete a key. Returns True if it existed.
    fn delete(&self, key: &str) -> bool {
        self.inner.remove(key).is_some()
    }

    /// Get all keys.
    fn keys(&self) -> Vec<String> {
        self.inner.iter().map(|e| e.key().clone()).collect()
    }

    /// Number of entries.
    fn __len__(&self) -> usize {
        self.inner.len()
    }

    /// Check if key exists.
    fn __contains__(&self, key: &str) -> bool {
        self.inner.contains_key(key)
    }

    /// dict-like: state["key"] = "value"
    fn __setitem__(&self, key: String, value: String) {
        self.set(key, value);
    }

    /// dict-like: state["key"]
    fn __getitem__(&self, key: &str) -> PyResult<String> {
        self.get(key)
            .ok_or_else(|| pyo3::exceptions::PyKeyError::new_err(key.to_string()))
    }

    /// dict-like: del state["key"]
    fn __delitem__(&self, key: &str) -> PyResult<()> {
        if self.delete(key) {
            Ok(())
        } else {
            Err(pyo3::exceptions::PyKeyError::new_err(key.to_string()))
        }
    }

    fn __repr__(&self) -> String {
        format!("SharedState({} keys)", self.inner.len())
    }
}
