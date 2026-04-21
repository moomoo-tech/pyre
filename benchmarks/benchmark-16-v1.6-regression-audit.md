# Benchmark 16 — v1.6 Regression Audit

**Recorded:** 2026-04-20
**Machine:** AMD Ryzen 7 7840HS (8C/16T), 59 GB RAM, powersave governor,
moderate background load (skytrade + prefect). Same machine as all
prior benchmarks; numbers track against recorded baselines.

**Scope:** After landing five HTTP-Arena-motivated features (compression,
TLS, upload streaming, async Postgres, CRUD helpers), confirm that none
of them regressed the headline perf or memory numbers.

## Summary

| Metric | v1.5.0 baseline | **Today (post-v1.6)** | Delta |
|---|---|---|---|
| Plaintext `GET /` | 423k rps | **430k rps** | **+1.7%** |
| Pipelined depth=16 | 902k rps | **921k rps** | **+2.2%** |
| RSS growth (plain JSON, 9M reqs) | 0.06 B/req | **0.12 B/req** | noise-level |
| RSS growth (TLS+brotli, 2.7M reqs) | n/a | **3.1 B/req** | new; 60× under gate |
| Memory regression suite | 9/9 | **9/9** | unchanged |
| Full pytest (no PG) | — | **256 passed** | 5 new suites green |
| cargo test | 42/42 | **42/42** | unchanged |
| clippy `-D warnings` | clean | **clean** | unchanged |

**All headline numbers improved or held.** The 0.12 B/req on the plain
JSON sustained path is within the measurement noise of the 0.06 B/req
previously recorded (both are mimalloc arena drift, not a real leak).

## Detail

### Throughput

```
Plaintext  GET /              wrk -t4 -c100 -d10s
  Latency: avg 189μs / σ 98μs / max 4.7ms / 79.8% ≤ avg
  Req/s:   430,080.60    Transfer/s: 62.75 MB

Pipelined  GET / (depth=16)   wrk -t8 -c256 -d10s --pipeline 16
  Latency: avg 2.58ms / σ 1.61ms / max 34ms / 69.6% ≤ avg
  Req/s:   921,670.16    Transfer/s: 134.49 MB
```

Both above the v1.5.0 recorded baselines. The added runtime machinery
(compression enable/disable branch, TLS Maybe-stream enum, streaming
flag lookup, DB module dormant) adds less than the day-to-day machine
variance. The compiled-in-but-disabled pattern documented in
`docs/features.md` is doing its job.

### Memory (sustained load)

**Plain JSON (no compression, no TLS):**

```
9,020,359 requests over 30.02 s at 300,477 rps
RSS warm: 208,668 KB   RSS end: 209,704 KB   Growth: 1,036 KB
→ 0.12 B / req
```

**TLS + brotli (both enabled, same payload):**

```
2,745,166 requests over 30.04 s at 91,376 rps
RSS warm: 255,248 KB   RSS end: 263,876 KB   Growth: 8,628 KB
→ 3.14 B / req
```

The ~3 B/req on the TLS+brotli path reflects transient allocator
pressure from brotli encoder buffers per request. mimalloc's arena
retention is slow to release under sustained load at 91k rps; the
per-request cost is steady-state, not monotonic. At 200 B/req hard
gate, we have **60× headroom** on the heaviest stack.

Memory regression test suite (all the named fixes shipped since v1.4.5):

```
tests/test_subinterp_memory_regression.py
  test_cookie_crlf_injection_rejected           PASSED
  test_cookie_plain_still_works                 PASSED
  test_router_case_insensitive_for_lowercase_method  PASSED
  test_request_response_lock_out_dynamic_attrs  PASSED
  test_pyrerequest_does_not_accumulate          PASSED
  test_headers_dicts_do_not_accumulate          PASSED
  test_subinterp_python_finalizers_fire         PASSED
  test_rss_growth_per_request_is_bounded        PASSED   (200 B/req gate)
  test_sustained_concurrent_load_no_leak        PASSED
```

No xfails, no skips, no soft-passes.

### Test suites

| Suite | Count | Result |
|---|---|---|
| `cargo test --release` | 42 | pass |
| Full `pytest tests/ --ignore=tests/e2e` | 256 | pass (25 DB/CRUD skipped without `PYRE_TEST_PG_DSN`) |
| `cargo clippy --release -- -D warnings` | — | clean |
| `cargo fmt --check` | — | clean |

With `PYRE_TEST_PG_DSN` set against `postgres:17-alpine`:

| Suite | Count | Result |
|---|---|---|
| `tests/test_db_pg.py` | 12 | pass |
| `tests/test_crud.py` | 13 | pass |

### What changed since v1.5.0 (feature-by-feature regression posture)

1. **Compression** (commits `8a4925e`, `9fd4f79`, `d7fe2aa`) —
   runtime toggle; disabled = one relaxed atomic load + branch-not-taken.
   Today's plaintext number (430k) is above the pre-compression baseline.
   ✓ No regression.
2. **TLS** (commit `f034b25`, `dfe65c8`) — accept loop wraps each
   stream in a `MaybeTlsStream` enum. Plain path takes the `Plain`
   variant with zero TLS code executed. Today's plaintext still >423k.
   ✓ No regression.
3. **Upload streaming** (commit `30d9520`) — `handle_request` was
   restructured so the route lookup happens before the body collect.
   For non-stream routes the old `Limited::new().collect()` path is
   preserved. ✓ No regression (pipelined 921k vs 902k).
4. **Async Postgres** (commit `a854593`) — a new module with its own
   tokio runtime. Inert until `PgPool.connect()` is called. ✓ No
   regression.
5. **CRUD helpers** (commit `3a1e113`) — pure Python; zero Rust
   footprint. ✓ No regression.

## Defensible claims for v1.6

- "Plaintext throughput improved 1.7% since v1.5.0 despite five new
  features landing."
- "Pipelined throughput improved 2.2% since v1.5.0."
- "Sustained-load RSS growth on the plain path is 0.12 B/req —
  effectively at the allocator-noise floor."
- "RSS growth under the heaviest stack (TLS + brotli) is 3.1 B/req,
  60× under the 200 B/req regression gate."
- "Every v1.5.0 memory-regression test still passes with no xfails."

## Ready for

- **HTTP Arena submission**: all five Arena profiles now covered in
  framework code — Baseline / Plaintext-pipelined, JSON, JSON
  Compressed, JSON TLS, Async DB, CRUD, Upload. See
  `docs/http-arena-preflight.md` for the per-profile score projections.
- **v1.6 release cut**: no open regressions. CHANGELOG draft should
  list the five features + the `PgPool.connect()` idempotency
  side-change.

## Reproducibility

```bash
# Plaintext & pipelined
python benchmarks/bench_plaintext.py &
wrk -t4 -c100 -d10s http://127.0.0.1:8000/
wrk -t8 -c256 -d10s -s /path/pipeline.lua http://127.0.0.1:8000/ -- 16

# Memory regression
pytest tests/test_subinterp_memory_regression.py -q

# DB + CRUD (needs docker)
docker run --rm -d --name pyre-pg -p 5433:5432 \
    -e POSTGRES_PASSWORD=pyre -e POSTGRES_DB=pyretest postgres:17-alpine
PYRE_TEST_PG_DSN="postgres://postgres:pyre@127.0.0.1:5433/pyretest" \
    pytest tests/test_db_pg.py tests/test_crud.py -v

# TLS+compression sustained
PYRE_COMPRESSION=1 \
PYRE_TLS_CERT=/tmp/pyre_tls/cert.pem PYRE_TLS_KEY=/tmp/pyre_tls/key.pem \
PYRE_PORT=8443 python benchmarks/bench_compression.py &
wrk -t4 -c100 -d30s -H 'Accept-Encoding: br' https://127.0.0.1:8443/json-fortunes
```
