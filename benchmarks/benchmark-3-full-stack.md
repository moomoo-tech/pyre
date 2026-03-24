# Full Stack Benchmark: Pyre vs Robyn vs Pure Rust (2026-03-23)

测试环境：macOS ARM64 (Apple Silicon), Python 3.14.3, Rust 1.93.1, wrk 4.2.0
参数：`wrk -t4 -c256 -d10s`

## 天梯排名

| 排名 | 框架 | GET / req/s | GET /hello req/s | avg 延迟 | 内存 (RSS) |
|------|------|------------|-----------------|----------|-----------|
| 1 | Actix-web (纯 Rust) | 226,042 | 208,796 | 1.00ms | 11 MB |
| 2 | Axum (纯 Rust, Pyre 同栈) | 223,845 | 220,891 | 0.86ms | 14 MB |
| 3 | **Pyre SubInterp** | **216,517** | **204,636** | **0.92ms** | **53 MB** |
| 4 | Pyre GIL | 100,673 | 97,727 | 2.56ms | 17 MB |
| 5 | Robyn --fast (22进程) | 76,504 | 76,051 | 29.62ms | 420 MB |

## Python 开销分析

| 对比 | GET / | GET /hello | 含义 |
|------|-------|-----------|------|
| Pyre SubInterp vs Axum (同栈) | 96.7% | 92.6% | Python 只损失 3-7% |
| Pyre SubInterp vs Actix | 95.8% | 98.0% | 几乎持平纯 Rust |
| Pyre GIL vs Axum | 45.0% | 44.2% | 单 GIL 砍掉一半性能 |
| Robyn --fast vs Actix (同栈) | 33.8% | 36.4% | 损失 2/3 性能 |

## 关键结论

1. **Pyre SubInterp 达到纯 Rust 93-97% 的性能** — Python handler 开销几乎可忽略
2. **瓶颈是 GIL 争用，不是 Python 执行速度** — 同样的 Python handler，SubInterp 比 GIL 模式快 2x
3. **Robyn 的 Actix-web 后端本身很快**（226k req/s），但 Python 层损耗了 2/3
4. **Pyre SubInterp 内存效率远超 Robyn** — 53 MB vs 420 MB，8x 更省
5. **纯 Rust Axum/Actix 天花板约 220k req/s** — Pyre SubInterp 已接近物理极限

## 技术栈对应关系

```
Pyre        = Tokio + Hyper + matchit   (≈ Axum)
Robyn       = Tokio + Actix-web         (≈ Actix-web)

Pyre SubInterp → 达到 Axum 97% 性能
Robyn --fast   → 达到 Actix 34% 性能
```
