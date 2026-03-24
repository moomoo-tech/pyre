# Benchmark 6: v0.4.0 全面压测 — spawn_blocking + 背压 + TCP_NODELAY (2026-03-24)

优化内容：spawn_blocking 隔离 GIL 调用、有界通道背压(503)、TCP_NODELAY、
orjson 自动检测、10MB body 上限、async def 检测、type stubs

测试环境：macOS ARM64 (Apple Silicon), Python 3.14.3, Rust 1.93.1, wrk 4.2.0
参数：预热 wrk -t2 -c50 -d3s, 正式 wrk -t4 -c256 -d10s

## 完整结果

| 场景 | Pyre SubInterp | Pyre Hybrid | Pyre GIL | Robyn --fast | 胜者 |
|------|----------------|-------------|----------|-------------|------|
| Hello World | 211,019 | **216,674** | 77,853 | 81,251 | Pyre **2.7x** |
| JSON small (3 fields) | **212,521** | 210,479 | 73,777 | 80,672 | Pyre **2.6x** |
| JSON medium (100 users) | 63,649 | **67,817** | 13,293 | 42,775 | Pyre **1.6x** |
| JSON large (500 records) | **5,063** | 4,835 | 1,917 | 4,467 | Pyre **13%** |
| fib(10) | 205,092 | **205,694** | 58,876 | 74,444 | Pyre **2.8x** |
| fib(20) | 10,830 | **11,065** | 1,899 | 8,664 | Pyre **28%** |
| fib(30) | **91** | 91 | 1 | 79 | Pyre **15%** |
| Pure Python sum(10k) | **74,894** | 74,362 | 18,036 | 43,836 | Pyre **1.7x** |
| sleep(1ms) | 7,867 | 7,905 | **46,704** | **77,967** | **Robyn 胜** |
| Parse 41B JSON | 203,262 | **208,249** | 60,514 | 75,277 | Pyre **2.8x** |
| Parse 7KB JSON | 90,867 | **94,942** | 19,873 | 50,520 | Pyre **1.9x** |
| Parse 93KB JSON | **9,958** | 9,750 | 1,847 | 7,345 | Pyre **36%** |
| numpy mean(10k) | — | 8,507 | 8,290 | **31,261** | **Robyn 胜** |
| numpy SVD 100x100 | — | 3,993 | 3,940 | **5,079** | **Robyn 胜** |

**总计：Pyre 胜 10/14 场景，Robyn 胜 3/14，持平 1/14**

## spawn_blocking 取舍分析

| 指标 | 优化前 (Round 1) | 优化后 (Round 2) | 变化 | 原因 |
|------|-----------------|-----------------|------|------|
| GIL Hello World | 119,049 | 77,853 | **-34%** | 每请求多一次线程切换 |
| GIL sleep(1ms) | 7,354 | 46,704 | **+535%** | 不再阻塞 Tokio worker |
| GIL numpy mean | 8,419 | 8,290 | -1.5% | numpy 本身是 CPU bound |
| SubInterp Hello | 219,210 | 211,019 | -3.7% | 正常波动 |
| Hybrid Hello | 217,610 | 216,674 | -0.4% | 不受影响 |

**结论**：GIL 模式纯吞吐量下降 34%（线程切换代价），但 I/O 并发提升 535%。
真实 Web 应用中 handler 必然包含 I/O（数据库、网络、文件），所以 spawn_blocking 是净收益。
SubInterp/Hybrid 模式完全不受影响（走 channel pool，不经过 spawn_blocking）。

## Pyre vs Robyn 对标分析

### Pyre 碾压 Robyn 的场景 (2-3x)

| 场景 | Pyre 优势 | 原因 |
|------|----------|------|
| Hello World | 2.7x | 子解释器真并行，零 GIL 争用 |
| fib(10) 轻 CPU | 2.8x | 10 个独立 GIL 并行计算 |
| JSON parse 41B | 2.8x | Rust 路由 + 子解释器并行 |
| sum(10k) 纯 Python | 1.7x | 多解释器消除 GIL 瓶颈 |

### Robyn 胜出的场景

| 场景 | Robyn 优势 | 原因 | Pyre 可改进方向 |
|------|----------|------|---------------|
| sleep(1ms) I/O | 10x | Robyn async + 多进程，Pyre sync handler | 支持 async handler |
| numpy mean | 3.7x | Robyn 多进程天然并行 numpy | 多进程模式 or ProcessPool |
| numpy SVD | 1.3x | 同上 | 同上 |

### 胜负平局的场景

| 场景 | 差距 | 分析 |
|------|------|------|
| JSON large (500 records) | Pyre +13% | 瓶颈在 Python JSON 序列化，两边差不多 |
| fib(30) 重 CPU | Pyre +15% | 完全 CPU bound，差距来自并行 worker 数量 |

## 架构对标：Pyre vs Robyn vs 理论上限

```
维度            Pyre (当前)              Robyn                下一步突破点
─────────────────────────────────────────────────────────────────────────
I/O 模型        Tokio (epoll)           Tokio (epoll)        io_uring (monoio)
并行模型        Sub-interpreter (OWN_GIL) 多进程              ← Pyre 独创优势
JSON 序列化     orjson 自动检测          Python json          SIMD-JSON (Rust)
GIL 策略        Per-Interpreter GIL     频繁获取/释放         ← Pyre 独创优势
WebSocket       tokio-tungstenite       actix-ws             已持平
HTTP 协议       HTTP/1.1                HTTP/1.1             HTTP/2, HTTP/3
路由匹配        matchit (编译时)         actix-router         已持平
背压            bounded channel + 503   无 (可能 OOM)        ← Pyre 优势
安全            10MB body + 路径穿越防御  未知                 ← Pyre 优势
```
