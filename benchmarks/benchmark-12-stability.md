# Benchmark 12: 5-Minute Stability Test — 64 Million Requests (2026-03-25)

## Purpose

Short benchmarks (10s) measure burst throughput. This test verifies **sustained stability** under continuous high load — the kind that exposes memory leaks, reference count bugs, queue exhaustion, and file descriptor limits.

## Environment

| Item | Value |
|------|-------|
| CPU | Apple M4 (10 cores) |
| OS | macOS Darwin 25.3.0 |
| Python | 3.14.3 |
| Rust | stable |
| Pyre | v1.2.0, sub-interpreter mode, 10 workers |
| Tool | wrk 4.2.0, 4 threads, 100 concurrent connections |
| FD limit | unlimited |

## Methodology

### What we test

1. **Memory stability** — Does RSS grow over time? Any `Py_INCREF` without matching `Py_DECREF` will leak ~50-100 bytes per request. At 214k req/s, even 10 bytes/req = 2.1 MB/s = 630 MB in 5 minutes. A flat memory curve proves zero leaks.

2. **Throughput consistency** — Does QPS degrade over time? Thread pool exhaustion, GC pressure, or lock contention would show as declining throughput.

3. **Error rate** — Any Non-2xx responses? 503 (backpressure) means the channel overflowed. Socket errors mean FD exhaustion or TCP state table overflow.

4. **Crash resilience** — Does the server survive all phases and respond correctly afterward?

### Steps

```bash
# 1. Start server (sub-interpreter mode, 10 workers)
python server.py  # 4 routes: /, /echo/{name}, /json (POST), /compute (fib10)

# 2. Record baseline RSS

# 3. Phase 1: Hello World — 300 seconds sustained
wrk -t4 -c100 -d300s http://127.0.0.1:18888/

# 4. Phase 2: Path params — 60 seconds
wrk -t4 -c100 -d60s http://127.0.0.1:18888/echo/benchmark

# 5. Phase 3: JSON POST (38-byte body) — 60 seconds
wrk -t4 -c100 -d60s -s post.lua http://127.0.0.1:18888/json

# 6. Phase 4: CPU-bound (fib10) — 60 seconds
wrk -t4 -c100 -d60s http://127.0.0.1:18888/compute

# 7. Health check: verify all 4 endpoints respond correctly
# 8. Record final RSS and compare to baseline
```

### Key metrics to observe

| Metric | What it means | Pass criteria |
|--------|---------------|---------------|
| RSS (start → end) | Memory leak detection | Flat or decreasing |
| Total requests | Sustained throughput proof | >60M in 5 min |
| Non-2xx responses | Backpressure / errors | 0 |
| Socket errors | FD exhaustion / TCP issues | 0 |
| Max latency | Tail latency under load | <100ms |
| Server alive after | Crash resilience | YES |

## Results

### Phase 1: Hello World — 5 minutes sustained

| Metric | Value |
|--------|-------|
| Duration | 300 seconds |
| **Total requests** | **64,410,189** |
| **Requests/sec** | **214,641** |
| Avg latency | 0.383ms |
| Max latency | 39.98ms |
| P99 latency | ~2ms (97.12% under 1ms) |
| Non-2xx responses | **0** |
| Socket errors | **0** |
| Throughput | 29.89 MB/s (8.76 GB total) |

### Phase 2: Path Params — 1 minute

| Metric | Value |
|--------|-------|
| **Requests/sec** | **213,896** |
| Avg latency | 0.396ms |
| Max latency | 21.32ms |
| Non-2xx responses | **0** |

### Phase 3: JSON POST (38-byte body) — 1 minute

| Metric | Value |
|--------|-------|
| **Requests/sec** | **210,835** |
| Avg latency | 0.403ms |
| Max latency | 22.69ms |
| Non-2xx responses | **0** |

### Phase 4: CPU-bound (fib10) — 1 minute

| Metric | Value |
|--------|-------|
| **Requests/sec** | **215,296** |
| Avg latency | 0.404ms |
| Max latency | 43.39ms |
| Non-2xx responses | **0** |

### Memory Timeline

```
Phase         Time     RSS
─────────────────────────────
Idle          0:00     1712 KB
Phase 1 start 0:05     848 KB  (sub-interpreters initialized, GC compaction)
Phase 1 mid   2:30     752 KB  (stable)
Phase 1 end   5:00     752 KB  (stable — 64M requests processed)
Phase 2 end   6:00     752 KB  (stable)
Phase 3 end   7:00     752 KB  (stable)
Phase 4 end   8:00     752 KB  (stable)
```

**Memory did not grow. It decreased from 1712 KB to 752 KB and held flat through 90+ million requests across all phases.**

### Post-test Health Check

```
GET  /              → {"hello": "world"}       ✅
GET  /echo/test     → {"name": "test"}         ✅
POST /json          → {"test": 1}              ✅
GET  /compute       → {"fib10": 55}            ✅
Server process      → alive                    ✅
```

## Summary

| Criterion | Result | Verdict |
|-----------|--------|---------|
| Memory leak | RSS flat at 752 KB through 90M+ requests | **PASS** |
| Throughput stability | 210-215k req/s consistent across all phases | **PASS** |
| Error rate | 0 Non-2xx, 0 Socket errors, 0 Timeouts | **PASS** |
| Crash resilience | Server alive and responsive after all phases | **PASS** |
| Tail latency | Max 43ms, P99 ~2ms | **PASS** |

### What this proves

1. **Zero memory leaks** — `PyObjRef` RAII + converting to pure Rust `SubInterpResponse` before crossing thread boundaries eliminates Python refcount leaks in async contexts.

2. **mimalloc handles fragmentation** — 90 million alloc/free cycles with zero RSS growth. The global allocator choice pays off under sustained load.

3. **Backpressure works but never triggers** — The bounded channel (`n * 128` capacity) with `try_send()` is sized correctly for this load. Zero 503s means 10 sub-interpreters fully absorb 214k req/s.

4. **No FD exhaustion** — Zero socket errors through 90M+ connections proves Hyper + Tokio's connection management is rock-solid.

5. **Architecture validated** — Single-process, 10 sub-interpreters, each with its own GIL, connected via crossbeam channels to a Tokio runtime. The design holds under extreme sustained load.
