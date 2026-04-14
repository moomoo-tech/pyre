//! SSE (Server-Sent Events) streaming response support.
//!
//! Handler returns a `PyreStream` object, then calls `stream.send("data")`
//! in a loop. Each send pushes a chunk to the HTTP response body.
//!
//! Resource lifecycle: `close()` performs deterministic channel teardown,
//! independent of Python GC timing. This prevents zombie TCP connections
//! when PyreStream is held by long-lived Python references.

use bytes::Bytes;
use pyo3::prelude::*;
use tokio::sync::mpsc;

/// Python-facing stream object. Handler calls send()/send_event()/close().
#[pyclass(frozen)]
pub(crate) struct PyreStream {
    // Wrapped in Option so close() can deterministically drop the Sender,
    // decoupling channel lifetime from Python GC (Haskell bracket pattern).
    tx: std::sync::Mutex<Option<mpsc::UnboundedSender<Result<Bytes, std::convert::Infallible>>>>,
    rx: std::sync::Mutex<Option<mpsc::UnboundedReceiver<Result<Bytes, std::convert::Infallible>>>>,
    /// Custom headers to include in the response
    #[pyo3(get)]
    pub(crate) content_type: String,
    #[pyo3(get)]
    pub(crate) status_code: u16,
    #[pyo3(get)]
    pub(crate) headers: std::collections::HashMap<String, String>,
}

#[pymethods]
impl PyreStream {
    /// Create a new SSE stream. Channel is created immediately so send() works right away.
    #[new]
    #[pyo3(signature = (content_type=None, status_code=200, headers=None))]
    fn new(
        content_type: Option<String>,
        status_code: u16,
        headers: Option<std::collections::HashMap<String, String>>,
    ) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        PyreStream {
            tx: std::sync::Mutex::new(Some(tx)),
            rx: std::sync::Mutex::new(Some(rx)),
            content_type: content_type.unwrap_or_else(|| "text/event-stream".to_string()),
            status_code,
            headers: headers.unwrap_or_default(),
        }
    }

    /// Send raw data chunk.
    fn send(&self, data: &str) -> PyResult<()> {
        let tx_guard = self.tx.lock().unwrap();
        if let Some(tx) = tx_guard.as_ref() {
            tx.send(Ok(Bytes::from(data.to_string())))
                .map_err(|_| pyo3::exceptions::PyConnectionError::new_err("client disconnected"))
        } else {
            Err(pyo3::exceptions::PyConnectionError::new_err(
                "stream was explicitly closed",
            ))
        }
    }

    /// Send an SSE event: `event: {event}\ndata: {data}\n\n`
    #[pyo3(signature = (data, event=None, id=None))]
    fn send_event(&self, data: &str, event: Option<&str>, id: Option<&str>) -> PyResult<()> {
        let mut msg = String::with_capacity(data.len() + 64);
        if let Some(id) = id {
            msg.push_str("id: ");
            msg.push_str(id);
            msg.push('\n');
        }
        if let Some(event) = event {
            msg.push_str("event: ");
            msg.push_str(event);
            msg.push('\n');
        }
        for line in data.lines() {
            msg.push_str("data: ");
            msg.push_str(line);
            msg.push('\n');
        }
        msg.push('\n'); // End of event
        self.send(&msg)
    }

    /// Deterministic channel teardown — drops the Sender immediately,
    /// causing the Tokio Receiver to see channel-closed and end the HTTP
    /// response. Does not depend on Python GC timing.
    fn close(&self) {
        if let Ok(mut lock) = self.tx.lock() {
            let _ = lock.take();
        }
    }
}

impl PyreStream {
    /// Take the receiver (called once by Rust handler to start streaming).
    pub(crate) fn take_rx(
        &self,
    ) -> Option<mpsc::UnboundedReceiver<Result<Bytes, std::convert::Infallible>>> {
        self.rx.lock().unwrap().take()
    }
}
