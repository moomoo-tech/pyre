# Changelog

## v1.4.0 (2026-04-01)

### Performance — Linux 42万 QPS
- **SO_REUSEPORT multi-accept** — N=io_workers 个独立 accept loop，Linux 内核级四元组哈希负载均衡，macOS 自动降级为 1
- **M:N scheduling** — `io_workers` (Tokio I/O threads) 和 `workers` (Python sub-interpreters) 独立配置，解耦网络层与计算层
- **LTO fat + codegen-units=1** — 编译期全局优化，+4% JSON/params 路由
- **TCP_QUICKACK** — Linux 禁用延迟 ACK，降低首字节延迟
- **Headers OnceLock lazy view** — 不访问 headers 时零开销，延迟转换
- **serde_json + pythonize** — Rust 侧 JSON 序列化，替代 Python json.loads
- **SharedState Bytes** — 零拷贝 clone
- **Arc\<str\> method/path** — 请求路径零分配
- **IpAddr lazy eval** — 不访问时不解析
- **Bytes zero-copy body** — 请求体零拷贝
- **mimalloc global allocator** — 高并发分配性能

### Features
- `io_workers` parameter — `app.run(workers=24, io_workers=16)` 或 `PYRE_IO_WORKERS=16`
- `client_ip` — 请求客户端 IP 地址
- Lifecycle hooks — `on_startup` / `on_shutdown`
- Zero-cost logging — Rust tracing engine + Python→Rust FFI bridge, OFF 级别原子跳过

### Benchmarks (Linux, AMD Ryzen 7 7840HS 8C/16T)
- **GET /: 420k req/s** (P99 571μs) — vs macOS v1.2.0 214k (+96%)
- **300s sustained: 401k req/s**, 1.2 亿请求, 0 错误, 内存仅 +27 MB
- **vs Robyn: 14-16x faster** across all routes

## v1.3.0 (2026-03-31)

### Features
- **Zero-cost logging system** — Rust `tracing` + `EnvFilter`, three targets, Python logging bridge via C-FFI
- **client_ip** property on PyreRequest
- **on_startup / on_shutdown** lifecycle hooks

### Performance
- IpAddr lazy evaluation
- Bytes zero-copy request body
- Arc\<str\> method/path to eliminate allocations
- Vec params (from HashMap)
- Zero-allocation hook iteration
- Sync Python log level with Rust EnvFilter

### Docs
- Sub-interpreter C extension compatibility guide (30/30 libs confirmed)
- English translations for all benchmark reports

## v1.2.0 (2026-03-25)

### Features
- **Dual async/sync worker pool** — `async def` handlers auto-route to asyncio event loops, `def` handlers to sync sub-interpreters. Zero config, zero performance loss.
- **Native async bridge (C-FFI)** — `pyre_recv`/`pyre_send` release GIL during channel wait, enabling true async in sub-interpreters.
- **MCP Server** — JSON-RPC 2.0 with `@app.mcp.tool()`, `@app.mcp.resource()`, `@app.mcp.prompt()` decorators.
- **MsgPack RPC** — `@app.rpc()` with content negotiation (MsgPack/JSON) + `PyreRPCClient` magic client.
- **SSE Streaming** — `PyreStream` with mpsc channel, returned directly from handlers.
- **SharedState** — Cross-worker `app.state` backed by `Arc<DashMap>`, nanosecond latency.
- **GIL Watchdog** — Monitor GIL contention, hold time, queue depth, memory RSS.
- **Backpressure** — Bounded channels with `try_send()`, returns 503 on overload.
- **Request timeout** — 30s zombie reaper in sub-interpreter mode (504 Gateway Timeout).
- **mimalloc** — Global allocator for high-concurrency allocation performance.
- **Hybrid dispatch** — `gil=True` routes auto-dispatch to main interpreter for C extension compatibility.

### Code Quality
- Extracted bootstrap script from Rust string to `python/pyreframework/_bootstrap.py` (`include_str!`).
- Removed dead `filter_script_ast` code.
- Moved CORS/logging from global statics to per-instance `PyreApp` fields.
- Added `debug_assert!(PyGILState_Check())` in `PyObjRef::Drop`.
- Full `cargo fmt` + zero clippy warnings.
- Migrated deprecated PyO3 `downcast` → `cast` calls.

### Bug Fixes
- **Fixed segfault on Ctrl+C** — `InterpreterPool::Drop` now joins worker threads before `Py_Finalize`.
- **Fixed KeyboardInterrupt noise** — Guard `signal.signal()` for main thread only.
- Hot reload fallback skips `.venv`/`node_modules`/`__pycache__`.

### Testing
- 21 Rust unit tests (response builders, MIME detection, header extraction, query params).
- 54 Python pytest tests (MCP, cookies, TestClient, RPC, static files, WebSocket, async isolation, logging).
- 22 integration tests (GIL + sub-interp modes, all features end-to-end).
- 5-minute stability benchmark: 64M requests, zero memory leaks, zero crashes.

### CI/CD
- GitHub Actions: cargo test → pytest → integration tests on Python 3.13/3.14.
- Blocking `cargo fmt --check` + `cargo clippy -- -D warnings`.

## v1.1.0 (2026-03-24)

### Features
- WebSocket support (text + binary) via tokio-tungstenite.
- Cookie utilities (`get_cookie`, `set_cookie`, `delete_cookie`).
- Multipart file upload parser.
- Redirect helper.
- TestClient for testing without a running server.
- Env var configuration (`PYRE_HOST`, `PYRE_PORT`, `PYRE_WORKERS`, `PYRE_LOG`).
- Hot reload (`reload=True` or `PYRE_RELOAD=1`).

## v1.0.0 (2026-03-23)

### Initial Release
- Rust core with Tokio + Hyper HTTP server.
- Per-Interpreter GIL (PEP 684) sub-interpreter pool.
- Decorator routing (`@app.get`, `@app.post`, etc.).
- Path params, query params, JSON parsing.
- CORS middleware.
- Static file serving with MIME detection + path traversal protection.
- Pydantic validation via `model=` parameter.
- Before/after request hooks.
- Graceful shutdown via Ctrl+C.
