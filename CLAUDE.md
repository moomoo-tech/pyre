# SkyTrade Engine (Pyre)

High-performance Python web framework powered by Rust. Goal: outperform Robyn.

## Architecture

- **Rust core** (`src/lib.rs`): Tokio + Hyper 1.x HTTP server, matchit router, PyO3 0.28 bridge
- **Python interface** (`python/skytrade/`):
  - `engine` (Rust): `SkyApp`, `SkyRequest`, `SkyResponse`
  - `app.py`: `Pyre` class — decorator-friendly wrapper over `SkyApp`
- **Build**: Maturin (mixed python/rust project), module name `skytrade.engine`

## Development

```bash
# Setup
python3 -m venv .venv && source .venv/bin/activate
pip install maturin

# Build (release mode)
maturin develop --release

# Run example
python examples/hello.py

# Benchmark (requires wrk: brew install wrk)
bash benchmarks/run_bench.sh
```

## Key Design Decisions

- Route table uses index-based lookup (Vec<Py<PyAny>> + Router<usize>) to avoid Py<PyAny> Clone issues in PyO3 0.28
- GIL released via `py.detach()` during Tokio event loop, reacquired via `Python::attach()` per-request for handler calls
- `#[pyclass(frozen)]` on SkyRequest/SkyResponse for thread safety — no mutable attributes
- `Pyre` Python wrapper provides decorator syntax; `SkyApp` is the raw Rust engine
- Static files served via Tokio async fs — no GIL needed
- Middleware: before_request/after_request hooks stored in RouteTable alongside route handlers

## Project Structure

```
src/lib.rs              # Rust HTTP server + PyO3 module
python/skytrade/
  __init__.py           # Re-exports Pyre, SkyApp, SkyRequest, SkyResponse
  app.py                # Pyre class (decorator syntax, logging, static files)
examples/hello.py       # Demo app with decorators + middleware
benchmarks/
  run_bench.sh          # Head-to-head benchmark vs Robyn
  robyn_app.py          # Robyn equivalent for comparison
  bench.py              # Standalone wrk runner
  benchmark-*.md        # Benchmark results history
```
