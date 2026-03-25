#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod app;
mod handlers;
mod interp;
mod json;
mod monitor;
mod response;
mod router;
mod state;
mod static_fs;
mod stream;
mod types;
mod websocket;

use pyo3::prelude::*;

#[pymodule]
fn engine(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<app::SkyApp>()?;
    m.add_class::<types::SkyRequest>()?;
    m.add_class::<types::SkyResponse>()?;
    m.add_class::<websocket::SkyWebSocket>()?;
    m.add_class::<state::SharedState>()?;
    m.add_class::<stream::SkyStream>()?;
    m.add_function(pyo3::wrap_pyfunction!(monitor::get_gil_metrics, m)?)?;
    Ok(())
}
