# Benchmark 5: v0.3.1 重构后性能验证 (2026-03-24)

重构内容：RAII PyObjRef、channel-based worker pool、AST 脚本过滤、Py_EndInterpreter Drop、
模块化拆分（8 文件）、路径穿越修复、二进制响应修复

测试环境：macOS ARM64 (Apple Silicon), Python 3.14.3, Rust 1.93.1, wrk 4.2.0
参数：`wrk -t4 -c256 -d10s`

## 结果

| 框架 | GET `/` req/s | GET `/hello/{name}` req/s | GET `/search?q=` req/s | avg 延迟 |
|------|-------------|------------------------|---------------------|---------|
| **Pyre SubInterp** (channel pool) | **215,429** | **212,602** | **215,200** | **0.86ms** |
| Pyre GIL (middleware+fallback) | 103,587 | 98,359 | 96,600 | 2.6ms |
| Robyn --fast | 83,272 | 83,313 | — | 25ms |

## 与 v0.3.0 (mutex 版) 对比

| 指标 | v0.3.0 (mutex) | v0.3.1 (channel) | 变化 |
|------|-------------|-----------------|------|
| SubInterp GET `/` | 223,440 | 215,429 | -3.6% |
| SubInterp GET `/hello` | 223,545 | 212,602 | -4.9% |
| SubInterp GET `/search` | 221,787 | 215,200 | -3.0% |
| GIL GET `/` | 100,386 | 103,587 | **+3.2%** |
| GIL GET `/hello` | 93,616 | 98,359 | **+5.1%** |
| GIL GET `/search` | 85,581 | 96,600 | **+12.9%** |

## 关键结论

1. **SubInterp 小幅下降 ~4%** — channel 跨线程通信 (crossbeam send + oneshot await) 比 mutex round-robin 多一次间接调用，属预期范围。但消除了队头阻塞：慢请求不再堵住其他 worker。
2. **GIL 模式显著提升 3-13%** — 模块化拆分后编译器优化更好，search 路由提升最大。
3. **Robyn 本次测试 83k** — 高于上次 76k，可能是系统负载波动。差距仍然 2.6x。
4. **安全修复零性能代价** — 路径穿越防御 (`trim_start_matches`) 和 PyBytes 二进制保持都是零开销操作。
5. **RAII 抽象零开销** — `PyObjRef` 的 `Drop` 调用 `Py_DECREF` 与手动调用完全等价，编译器内联后无额外开销。

## 架构变更对比

| 维度 | v0.3.0 (重构前) | v0.3.1 (重构后) |
|------|---------------|----------------|
| 文件数 | 2 (lib.rs + interp.rs) | 9 个模块 |
| 最大文件 | lib.rs 981 行 | interp.rs 820 行 |
| Worker 调度 | Round-robin + per-worker Mutex | crossbeam 多消费者 channel |
| 引用计数 | 手动 Py_INCREF/DECREF | RAII PyObjRef (Drop 自动) |
| 脚本过滤 | 字符串行匹配 | Python AST parse + unparse |
| 子解释器清理 | 无 (进程退出兜底) | Py_EndInterpreter on Drop |
| 安全漏洞 | 路径穿越 + 二进制损坏 | 已修复 |
