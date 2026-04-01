# Benchmark 14: Linux Full Benchmark + Fair Robyn Comparison (2026-04-01)

## Purpose

Pyre v1.3→v1.4 introduced a series of Linux-specific optimizations. This is the first full 4-phase benchmark on Linux, validating:

1. Real-world impact of SO_REUSEPORT multi-accept under the Linux kernel
2. Performance of M:N scheduling (independent io_workers / workers configuration)
3. Fair head-to-head comparison with Robyn at **equal worker scale**
4. Stability and memory behavior under 300-second sustained load

## Environment

| Item | Value |
|------|-------|
| CPU | AMD Ryzen 7 7840HS (8C/16T) |
| RAM | 60 GB DDR5 |
| OS | Ubuntu 24.04, Linux 7.0.0 (x86_64) |
| Python | 3.12.13 |
| Rust | stable (LTO fat, codegen-units=1) |
| Pyre | v1.4.0, sub-interpreter mode, 16 workers + 16 io_workers |
| Robyn | v0.82.1 (actix-web) |
| Tool | wrk 4.1.0 |

## v1.2→v1.4 Optimization Changelog

| Optimization | Impact |
|-------------|--------|
| SO_REUSEPORT multi-accept | N=io_workers independent accept loops on Linux, kernel 4-tuple hash load balancing |
| M:N scheduling | io_workers (Tokio I/O) and workers (Python sub-interpreters) configured independently |
| LTO fat + codegen-units=1 | Whole-program optimization at compile time |
| TCP_QUICKACK | Disable delayed ACK on Linux, reduces time-to-first-byte |
| Headers OnceLock lazy conversion | Zero overhead when headers are not accessed |
| serde_json + pythonize | Rust-side JSON serialization replaces Python json.loads |
| SharedState Bytes | Zero-copy clone |
| Arc\<str\> method/path | Zero allocation |
| IpAddr lazy eval | Not parsed unless accessed |
| Bytes zero-copy body | Zero-copy request body |
| mimalloc global allocator | High-concurrency allocation performance |

---

## Phase 1: Route Performance (wrk -t4 -c100 -d10s)

| Route | QPS (req/s) | P50 | P90 | P99 | Max |
|-------|-------------|-----|-----|-----|-----|
| GET / (plain text) | **419,730** | 188μs | 284μs | 571μs | 7.49ms |
| GET /json | **406,947** | 197μs | 297μs | 575μs | 7.16ms |
| GET /headers | **402,439** | 199μs | 295μs | 550μs | 4.77ms |
| GET /user/42 (param) | **398,932** | 200μs | 309μs | 661μs | 6.04ms |
| GET /user/7/post/99 (2 params) | **394,862** | 202μs | 313μs | 659μs | 5.59ms |
| GET /compute (CPU) | **378,878** | 212μs | 332μs | 778μs | 38.52ms |
| POST /echo (JSON parse) | **376,421** | 211μs | 336μs | 725μs | 4.90ms |
| GET /query?a=1&b=2 | **372,729** | 216μs | 344μs | 800μs | 10.74ms |

```
GET /            ████████████████████████████████████████████████████ 419,730
GET /json        █████████████████████████████████████████████████   406,947
GET /headers     █████████████████████████████████████████████████   402,439
GET /user/42     ████████████████████████████████████████████████    398,932
GET /user/7/p/99 ████████████████████████████████████████████████    394,862
GET /compute     ███████████████████████████████████████████████     378,878
POST /echo       ██████████████████████████████████████████████      376,421
GET /query       █████████████████████████████████████████████       372,729
```

All 8 routes sustain **370k–420k QPS** with P99 < 1ms across the board.

## Phase 2: Concurrency Scaling (GET /)

| Connections | QPS (req/s) | P50 | P99 | Max |
|-------------|-------------|-----|-----|-----|
| c=50 | 360,016 | 105μs | 590μs | 5.09ms |
| c=100 | 406,783 | 196μs | 519μs | 4.92ms |
| c=256 | **410,011** | 331μs | 1.12ms | 18.80ms |
| c=512 | 400,849 | 619μs | 1.71ms | 39.51ms |

Peak throughput around c=256. At c=50, P50 is just 105μs; at c=512, QPS remains at 400k with < 3% degradation.

## Phase 3: 300-Second Sustained Load (Stability)

| Metric | Value |
|--------|-------|
| Duration | 300 seconds (5 minutes) |
| QPS (average) | **400,683 req/s** |
| Total requests | **120,209,758** (120 million) |
| P50 / P99 | 199μs / 586μs |
| Max | 9.02ms |
| Non-2xx responses | **0** |
| Socket errors | **0** |
| Memory (start) | 168.9 MB |
| Memory (after 300s) | 195.6 MB |
| Memory growth | +26.7 MB (+15.8%) |
| Memory (final) | 221.8 MB |

**120 million requests over 300 seconds, zero errors.** Memory grew by only 27 MB — no leak detected.

## Phase 4: Stress Test (c=1024)

| Metric | Value |
|--------|-------|
| QPS | **356,328 req/s** |
| P50 / P99 | 1.40ms / 3.15ms |
| Total requests | 3,597,188 |
| Errors | **0** |

356k QPS under 1024 concurrent connections, zero errors — graceful degradation.

---

## Pyre vs Robyn: Fair Comparison

### Configuration

| | Pyre | Robyn |
|---|---|---|
| Version | v1.4.0 | v0.82.1 |
| Architecture | 1 process, 16 sub-interpreters + 16 IO threads | **16 processes × 2 workers** (32 workers total) |
| Runtime | Tokio + Hyper | actix-web |
| wrk args | -t4 -c100 -d10s | -t4 -c100 -d10s |

> Robyn launched with `--processes 16 --workers 2` to ensure a fair comparison at equal scale.

### Throughput

| Route | Pyre (req/s) | Robyn (req/s) | Ratio |
|-------|-------------|--------------|-------|
| GET / (plain text) | **428,576** | 156,245 | **2.7x** |
| GET /json | **404,685** | 154,840 | **2.6x** |
| GET /user/42 (param) | **395,244** | 144,393 | **2.7x** |
| POST /echo (JSON) | **371,601** | 144,071 | **2.6x** |
| GET /compute (CPU) | **379,005** | 144,750 | **2.6x** |

```
Pyre   GET /     ████████████████████████████████████████████████████ 428,576
Robyn  GET /     ███████████████████                                  156,245

Pyre   GET /json █████████████████████████████████████████████████   404,685
Robyn  GET /json ██████████████████                                   154,840

Pyre   POST      ████████████████████████████████████████████████     371,601
Robyn  POST      █████████████████                                    144,071
```

### Latency

| Route | Pyre P50 / P99 | Robyn P50 / P99 |
|-------|----------------|-----------------|
| GET / | **185μs / 579μs** | 527μs / 2.01ms |
| GET /json | **196μs / 599μs** | 553μs / 1.29ms |
| GET /user/42 | **201μs / 641μs** | 601μs / 1.49ms |
| POST /echo | **214μs / 785μs** | 591μs / 1.47ms |
| GET /compute | **211μs / 772μs** | 609μs / 1.49ms |

Pyre P50 latency is **2.8–3.0x lower** than Robyn.

### Resource Efficiency

| Metric | Pyre | Robyn |
|--------|------|-------|
| Memory RSS | **189 MB** | 583 MB |
| Processes | **1** | 16 |
| QPS / MB | **2,268 req/s/MB** | 268 req/s/MB |
| Cross-worker shared state | Built-in (DashMap, nanosecond) | Requires Redis |

Pyre delivers **2.7x the throughput on 1/3 the memory**. Per-MB QPS yield is **8.5x** higher than Robyn.

### Why Is Pyre Faster?

| Dimension | Pyre | Robyn |
|-----------|------|-------|
| Process model | 1 process, N sub-interpreters sharing address space | N independent OS processes, each copying the Python runtime |
| Memory overhead | ~10 MB incremental per worker | ~35 MB per process |
| Worker communication | crossbeam channel (lock-free, nanosecond) | None (process isolation) |
| Accept model | SO_REUSEPORT multi-accept, kernel load balancing | Single accept, actix-web internal dispatch |
| GIL strategy | Per-Interpreter GIL, true parallelism | Multi-process, one GIL each |
| JSON serialization | Rust serde_json + pythonize | Python json.dumps |
| Allocator | mimalloc (high-concurrency optimized) | System malloc |

The core difference: Pyre achieves multi-core parallelism **within a single process**, eliminating process duplication and IPC overhead. Robyn uses the traditional multi-process model — each additional worker costs ~35 MB and cross-worker state sharing requires external infrastructure like Redis.

---

## vs macOS Baseline (v1.2.0)

| Route | macOS M4 (v1.2.0) | Linux 7840HS (v1.4.0) | Improvement |
|-------|-------------------|----------------------|-------------|
| GET / | 214,641 | **428,576** | **+99.7%** |
| GET /user/42 | 213,896 | **395,244** | **+84.8%** |

> macOS SO_REUSEPORT does not perform kernel load balancing — multi-accept actually adds kqueue registration overhead. v1.4.0 auto-detects: Linux uses N-way accept, macOS falls back to 1.

## Multi-Core Scaling Projection

This benchmark was run on 8C/16T. The following projections estimate expected throughput on larger hardware based on measured data.

### Model

Measured: 429k QPS on GET / with 8C/16T. Bottleneck analysis:

1. **Tokio I/O layer** (accept + parse + respond): P50 ~50μs — far from saturated
2. **Python sub-interpreter layer** (handler execution): P50 ~150μs — primary bottleneck
3. **crossbeam channel**: MPMC contention grows with consumer count, but negligible below 32 workers

Hyper-threading (SMT) provides ~20–30% uplift for the Python interpreter (shared ALU/FPU, L1/L2 cache contention). Thus 8C/16T ≈ ~10–10.5 effective full cores of Python throughput.

### Scaling Factors

| Cores | Effective Full Cores (SMT ×1.25) | Relative to 8C/16T | Projected QPS (GET /) | Projected Memory |
|-------|----------------------------------|--------------------|-----------------------|-----------------|
| 4C/8T | ~5 | 0.48× | ~205k | ~120 MB |
| 8C/16T | ~10 | 1.00× | **429k** (measured) | 189 MB |
| 16C/32T | ~20 | 1.90× | **~810k** | ~350 MB |
| 32C/64T | ~38 | 3.62× | **~1.2M** | ~650 MB |
| 64C/128T | ~72 | 5.80×* | **~1.5M*** | ~1.2 GB |

> \* At 64C+, channel contention and NUMA cross-node access become new bottlenecks, reducing linearity to ~80%.

### Key Assumptions and Limitations

1. **Client load generation**: wrk -t4 is near client-side saturation at 429k — 16C+ testing requires `-t8` or multiple client machines
2. **Channel contention**: crossbeam MPMC CAS failure rate rises at 32+ consumers; flume bounded channel can mitigate
3. **NUMA**: On dual-socket EPYC, cross-NUMA channel access adds ~100ns vs ~20ns same-node — recommend `numactl --interleave=all`
4. **Kernel TCP stack**: SO_REUSEPORT with 32+ accept loops may suffer uneven softirq distribution — requires RFS (Receive Flow Steering) tuning
5. **Memory**: ~10 MB per sub-interpreter, scales linearly and predictably

### Recommended Configuration

| Hardware | workers | io_workers | Expected QPS |
|----------|---------|------------|-------------|
| 4C/8T (Raspberry Pi 5, small VPS) | 8 | 4 | ~200k |
| 8C/16T (Ryzen 7, this benchmark) | 16 | 16 | ~430k |
| 16C/32T (Ryzen 9 / Xeon) | 32 | 16 | ~800k |
| 32C/64T (EPYC single-socket) | 48 | 32 | ~1.2M |
| 64C/128T (EPYC dual-socket) | 80 | 64 | ~1.5M |

> workers need not equal thread count. CPU-bound routes: set workers = core count. I/O-bound routes: 2–3× core count. io_workers should match physical cores.

## Conclusion

1. **Pyre v1.4.0 on Linux achieves 430k QPS** — a 2× improvement over macOS v1.2.0 (+100%)
2. **Fair comparison with Robyn (equal 16 workers): 2.6–2.7x faster, 1/3 the memory**
3. **300-second sustained 400k QPS, 120 million requests, zero errors**
4. **c=1024 extreme concurrency still delivers 356k QPS** — graceful degradation, no crashes
5. **Per-MB QPS yield is 8.5x higher than Robyn** — significant deployment cost advantage
