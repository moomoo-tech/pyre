# Phase 2 + Phase 4 Benchmark Results (2026-03-23)

测试环境：macOS ARM64 (Apple Silicon), Python 3.14.3, Rust 1.93.1, wrk 4.2.0
参数：`wrk -t4 -c256 -d10s`

## 四种模式完整对比

### 吞吐量 (req/s)

| 模式 | GET / | GET /hello/bench | 备注 |
|------|-------|-----------------|------|
| **Pyre SubInterp** | **213,759** | **198,441** | 10 个子解释器，各自独立 GIL |
| Pyre GIL | 95,926 | 91,722 | 普通 Python 3.14，单 GIL |
| Pyre NO-GIL | 95,374 | 86,182 | Free-threaded Python 3.14t |
| Robyn --fast | 71,032 | 70,440 | 22 进程多 worker 模式 |

### 延迟 (avg)

| 模式 | GET / | GET /hello/bench |
|------|-------|-----------------|
| **Pyre SubInterp** | **0.91ms** | **1.06ms** |
| Pyre GIL | 2.85ms | 2.86ms |
| Pyre NO-GIL | 2.83ms | 3.34ms |
| Robyn --fast | 22.23ms | 29.13ms |

### 内存用量 (RSS)

| 模式 | 空闲 | 峰值 (256并发) | 压测后 |
|------|------|---------------|--------|
| Pyre GIL | 10 MB | 16 MB | 16 MB |
| Pyre NO-GIL | 18 MB | 29 MB | 29 MB |
| **Pyre SubInterp** | **52 MB** | **67 MB** | **67 MB** |
| Robyn --fast | 437 MB | 451 MB | 432 MB |

## 关键对比：Pyre SubInterp vs Robyn --fast

| 指标 | Pyre SubInterp | Robyn --fast | 倍数 |
|------|---------------|-------------|------|
| GET / 吞吐 | 213,759 req/s | 71,032 req/s | **3.0x** |
| GET /hello 吞吐 | 198,441 req/s | 70,440 req/s | **2.8x** |
| GET / 延迟 | 0.91ms | 22.23ms | **24x 更低** |
| 峰值内存 | 67 MB | 451 MB | **6.7x 更省** |

## 分析

### 为什么子解释器模式最快
- 10 个子解释器各自持有独立 GIL (OWN_GIL)，真正并行执行 Python handler
- 同一进程内，共享 heap/code pages，内存效率远高于多进程
- 每个子解释器 ~5 MB 额外开销，而 Robyn 每个进程 ~20 MB

### 为什么 NO-GIL 没有预期快
- Free-threaded Python 的原子操作和引用计数开销抵消了部分并行收益
- 对于简单 handler（几微秒），GIL 竞争本身不是主要瓶颈
- 子解释器完全隔离，无共享状态竞争，反而更快

### 内存效率
- Pyre SubInterp 峰值 67 MB 跑出 213k req/s = **3,184 req/s/MB**
- Robyn --fast 峰值 451 MB 跑出 71k req/s = **157 req/s/MB**
- **Pyre 内存效率是 Robyn 的 20 倍**
