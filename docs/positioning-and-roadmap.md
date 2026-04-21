# Pyronova — Positioning & Roadmap

Checkpoint as of 2026-04-21. Read this first in any new session to
understand what Pyronova is, what it isn't, and what's worth working on.

## Identity

Pyronova is **a self-contained high-performance Python web framework with a
Rust core**. It is NOT an ASGI-compatible FastAPI replacement. It is NOT
a Django-style batteries-included monolith. It is not trying to win
every HTTP Arena profile at any cost.

Decorator-style API (`@app.get("/")`) with Pyronova-specific `Request` /
`Response` types. Users who want Starlette/FastAPI middleware use
Starlette/FastAPI — not Pyronova. Users who want a direct Python-with-Rust-core
path to real multi-core throughput (PEP 684 sub-interpreters, no GIL
contention, low memory footprint) pick Pyronova.

Explicit anti-goals:
- **ASGI/WSGI protocol compatibility.** The scope/receive/send
  translation layer costs ~μs per request and forces every Pyronova
  primitive (streaming, compression, fast_response) through an
  abstraction that wasn't built for them.
- **Winning every HTTP Arena profile.** We accept losing pipelined
  and low-per-request JSON to pure-Rust frameworks — the cost of
  being Python-facing is real. We aim for the profiles where a
  thin Python dispatch path does best: static, json-comp, async-db
  tuned, upload.
- **Drop-in compatibility with FastAPI routes.** Pydantic integration
  stays opt-in; most users write plain `def handler(req): return {...}`.

Where we DO want to win hard:
- **Python framework #1 on baseline, short-lived, upload** (per
  HTTP Arena's 64C projections).
- **Static file serving** — pure Rust path, never enters Python.
  Currently 13× Actix on HTTP Arena static profile.
- **Memory stability** — 0.12 B/req sustained on clean paths,
  structural DoS protection (TLS timeout, body admission, bounded
  stream channels).

## Current shape (v1.5 shipped, v1.6 in progress)

| Subsystem | State |
|---|---|
| Core routing + sub-interp dispatch | stable |
| TLS (rustls) | opt-in, H2 via ALPN |
| Compression (gzip + brotli) | opt-in, configurable quality |
| Streaming uploads | opt-in per route, bounded bp |
| Postgres sync + async API | `pyronova.db.PgPool` |
| CRUD REST helper | `pyronova.crud.register_crud` |
| Fast-path routes (no Python) | `app.add_fast_response` |
| WebSocket | stable |
| SSE (Stream) | stable |
| MCP server | stable |
| Static files | stable |

## What the audit produced (5 rounds, Feb-Apr 2026)

28 bug claims evaluated. **10 real structural bugs fixed** (tests in `tests/`):

- CORS 404/static short-circuit (round 3)
- Zombie worker cross-pool request theft (round 3)
- Sub-interp UAF on drop (round 3)
- DB decode fallback explosion on UUID/TIMESTAMP (round 3)
- TLS Slowloris (round 4)
- Stream body unbounded OOM (round 4)
- PyErr_Print stderr serialization storm (round 4)
- Async worker shutdown without task cleanup (round 4)
- Eager-eval DoS (body before permit) (round 5)
- FFI panic UB / abort (round 5)

18 rejected with justification. Pattern:
- **Structural-gap claims** ("resource without timeout/bound/admission")
  hit ~60%+
- **Call-site behavior claims** ("UB on Drop", "GIL blackhole",
  "cache coherency across interpreters") hit <20% — pyo3 and our
  existing architecture handles most of them

Methodology ran out of signal at round 5. The next batch was 0/6 real.

Defense primitives kept around for future reuse:
- `log_and_clear_py_exception(context)` — exception → tracing
- `ffi_catch_unwind(context, fn)` — wrap all FFI entry points
- `InterpreterPool::submit_semaphore` — admission control
- `WorkerState::pool_id` + `_pyronova_pool_id` — zombie-worker guard
- `MaybeTlsStream` enum — plain/TLS unified IO type

## Open engineering work

Ranked by real-user ROI, not benchmark ROI.

### P1 — ship blockers for v1.6

1. **`fetch_iter` streaming DB cursor.** O(2N) peak memory on
   50k+ row exports. Real user pain. Design: sqlx `fetch()` + Pin<Box<Stream>>
   + Mutex + `rt.block_on` per step. New API; doesn't break `fetch_all`.
2. **Release cut.** Tag v1.6, write CHANGELOG (11 features since
   v1.5 + 10 hardenings from audit), push PyPI.

### P2 — production-grade polish (what the "is this useful" thread
surfaced)

3. **Observability.** Structured tracing span per request, trace-id
   header propagation (`X-Request-ID`), Prometheus `/metrics` endpoint.
4. **Health-check protocol.** `/livez` / `/readyz` + startup probes
   (DB ping, migration check).
5. **Unified config.** Pydantic-settings style, replaces scattered
   `app.enable_X()` chain in user scripts.
6. **Error pages.** Pluggable templates for 500 / 404; default
   JSON stays but users can override to HTML.
7. **Request context.** `req.trace_id`, `req.span` that propagate
   into `pool.fetch_one` log lines automatically.

### P3 — benchmark / marketing

8. **HTTP Arena submission PR** to `MDA2AV/HttpArena`. Artifacts in
   `arena_submission/`. Benchmark-17 has the numbers. Clean-room
   rerun + PR.
9. **Comparative bench docs.** `pyronova vs fastapi vs uvicorn vs robyn
   vs fastpysgi` on this machine. FastAPI 5× lead is the main claim.

### P4 — ecosystem

10. **TestClient v2.** pytest plugin, fixture factories for common
    setups (TLS, auth, DB seed).
11. **Docs reshape.** Tutorial + API reference + deployment guide
    (parallel to uvicorn's docs). Current `docs/*.md` is mostly
    post-mortems and design notes — good for contributors, bad for
    new users.
12. **CLI.** `pyronova run app:app`, `pyronova dev` with reload, `pyronova
    routes` to list the registered routes.

## Anti-features (actively won't do)

- ASGI/WSGI protocol adapter (see Identity). Users who want those
  use `uvicorn` or `gunicorn`.
- Pipelined-at-any-cost tricks (caching the `/pipeline` route in
  Rust, etc.). Robyn got 16M rps on pipelined by not entering
  Python; Pyronova's pipelined passes through Python as the contract.
  We accept losing that profile.
- Drop-in FastAPI route compat layer. Too much scope creep; FastAPI
  users are better served by staying on FastAPI.
- C-extension sandbox / process isolation. We use
  `check_multi_interp_extensions=1` which strictly rejects
  non-PEP-684-safe libs. Users see `ImportError` at startup, not
  runtime segfaults. That's the trade we accept.

## Development rhythm

- Audit hit rate trajectory tells us when to stop: >40% real → keep
  going; <20% → stop, false-positive noise exceeds real signal.
- Every fix lands with at least one regression test in `tests/` or a
  `cargo test` case in `src/*.rs`.
- Full pytest (270+ tests) + cargo test (44 tests) + clippy
  `-D warnings` must be green before push.
- memory-regression suite (9 tests, 200 B/req hard gate) stays green.

## Session-handoff summary

- **Last landed:** `d9e9f75` (round-5 fixes: admission semaphore,
  FFI catch_unwind)
- **Suite status:** 275 pytest pass / 30 skipped (no-PG), 44 cargo
  test pass, clippy clean
- **Bench status:** benchmark-17 pinned Pyronova composite ~620 on 8C
  Ryzen in Arena harness; projected 64C rank #1 Python on baseline /
  short-lived / upload
- **Next action:** pick P1 item (`fetch_iter` or release cut) and go
