# Benchmark 14: Linux v1.3.0 全量压测 + Robyn 对比 (2026-04-01)

## 目的

在 Linux 上验证 v1.3.0 全部优化（SO_REUSEPORT 多路 Accept、M:N 调度、LTO、TCP_QUICKACK、serde_json/pythonize、OnceLock headers、mimalloc）的综合表现，并与 Robyn 做 head-to-head 对比。

## 环境

| 项目 | 值 |
|------|------|
| CPU | AMD Ryzen 7 7840HS (8C/16T) |
| RAM | 60 GB |
| OS | Ubuntu 24.04, Linux 7.0.0-10-generic (x86_64) |
| Python | 3.12.13 |
| Rust | stable (LTO fat, codegen-units=1) |
| Pyre | v1.3.0, sub-interpreter 模式, 16 workers, 16 IO threads |
| Robyn | 0.82.1 (actix-web, 1 worker) |
| 工具 | wrk 4.1.0, 4 threads |

## Phase 1: 短爆发 (c=100, 10s/路由)

| 路由 | QPS (req/s) | P50 | P75 | P90 | P99 | Max |
|------|-------------|-----|-----|-----|-----|-----|
| GET / (plain text) | **419,730** | 188μs | 225μs | 284μs | 571μs | 7.49ms |
| GET /json | **406,947** | 197μs | 233μs | 297μs | 575μs | 7.16ms |
| GET /user/42 (path param) | **398,932** | 200μs | 237μs | 309μs | 661μs | 6.04ms |
| GET /user/7/post/99 (2 params) | **394,862** | 202μs | 240μs | 313μs | 659μs | 5.59ms |
| POST /echo (JSON parse+serialize) | **376,421** | 211μs | 251μs | 336μs | 725μs | 4.90ms |
| GET /headers (header access) | **402,439** | 199μs | 235μs | 295μs | 550μs | 4.77ms |
| GET /query?a=1&b=2 (query params) | **372,729** | 216μs | 256μs | 344μs | 800μs | 10.74ms |
| GET /compute (CPU-bound) | **378,878** | 212μs | 250μs | 332μs | 778μs | 38.52ms |

### 吞吐量对比

```
GET /             ████████████████████████████████████████████████████ 419,730 req/s
GET /json         ██████████████████████████████████████████████████   406,947 req/s
GET /user/42      █████████████████████████████████████████████████    398,932 req/s
GET /user/7/p/99  ████████████████████████████████████████████████     394,862 req/s
POST /echo        ███████████████████████████████████████████████      376,421 req/s
GET /headers      █████████████████████████████████████████████████    402,439 req/s
GET /query        ██████████████████████████████████████████████       372,729 req/s
GET /compute      ███████████████████████████████████████████████      378,878 req/s
```

## Phase 2: 并发阶梯 (GET /, 10s)

| 并发 | QPS (req/s) | P50 | P99 | Max |
|------|-------------|-----|-----|-----|
| c=50 | 360,016 | 105μs | 590μs | 5.09ms |
| c=100 | 406,783 | 196μs | 519μs | 4.92ms |
| c=256 | **410,011** | 331μs | 1.12ms | 18.80ms |
| c=512 | 400,849 | 619μs | 1.71ms | 39.51ms |

最佳吞吐在 c=256 附近达到，之后 P99 上升但 QPS 保持稳定。

## Phase 3: 300s 持续负载 (c=100, GET /)

| 指标 | 值 |
|------|------|
| QPS | **400,683 req/s** |
| Total requests | **120,209,758** |
| P50 | 199μs |
| P99 | 586μs |
| Max | 9.02ms |
| Errors | **0** |
| Memory (start) | 168.9 MB |
| Memory (300s后) | 195.6 MB |
| Memory (最终) | 221.8 MB |

300s 持续 40 万 QPS，零错误，内存从 169MB 缓增到 196MB（+16%），无泄漏迹象。

## Phase 4: 压力测试 (c=1024, 10s)

| 指标 | 值 |
|------|------|
| QPS | **356,328 req/s** |
| P50 | 1.40ms |
| P99 | 3.15ms |
| Total | 3,597,188 |
| Errors | **0** |

1024 并发下仍有 35.6 万 QPS，零错误。

## Pyre vs Robyn 对比 (c=100, 10s)

| 路由 | Pyre (req/s) | Robyn (req/s) | 倍率 |
|------|-------------|--------------|------|
| GET / | **419,793** | 28,153 | **14.9x** |
| GET /json | **405,104** | 26,283 | **15.4x** |
| GET /user/42 | **401,390** | 26,646 | **15.1x** |
| GET /user/7/post/99 | **402,634** | 25,884 | **15.6x** |
| POST /echo | **378,299** | 27,733 | **13.6x** |
| GET /headers | **409,239** | 6,378 | **64.2x** |
| GET /compute | **388,314** | 24,754 | **15.7x** |

### 延迟对比

| 路由 | Pyre P50 | Pyre P99 | Robyn P50 | Robyn P99 |
|------|----------|----------|-----------|-----------|
| GET / | 189μs | 602μs | 3.39ms | 5.06ms |
| GET /json | 198μs | 611μs | 3.62ms | 5.72ms |
| GET /user/42 | 199μs | 651μs | 3.53ms | 5.65ms |
| POST /echo | 210μs | 744μs | 3.46ms | 5.40ms |
| GET /headers | 196μs | 515μs | 15.51ms | 19.61ms |
| GET /compute | 207μs | 759μs | 3.81ms | 6.26ms |

### 内存对比

| 框架 | Memory |
|------|--------|
| Pyre | 188.2 MB |
| Robyn | 37.6 MB |

> 注: Robyn 使用 1 个 actix-web worker，Pyre 使用 16 sub-interpreters + 16 IO threads。Robyn 内存更低但吞吐量和延迟差距巨大。

## vs macOS 基线 (v1.2.0)

| 路由 | macOS v1.2.0 | Linux v1.3.0 | 提升 |
|------|-------------|-------------|------|
| GET / | 214,641 | **419,730** | **+96%** |
| GET /user/42 | 213,896 | **398,932** | **+87%** |

## 结论

1. **Linux 上 Pyre v1.3.0 达到 42 万 QPS**，比 macOS v1.2.0 基线提升 ~96%
2. **SO_REUSEPORT 多路 Accept** 在 Linux 内核上发挥了真正的负载均衡效果
3. **vs Robyn 13.6-15.7x 吞吐量优势**，延迟低 18-80x
4. 300s 持续负载零错误，内存稳定
5. c=1024 极端压力下仍保持 35.6 万 QPS
6. M:N 调度模型（16 IO + 16 Python workers）在 8C/16T 上表现最优
