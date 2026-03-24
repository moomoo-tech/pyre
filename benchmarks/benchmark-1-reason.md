# Phase 1 Benchmark 分析：为什么 SkyTrade 比 Robyn 快 2.5x

## 测试结果回顾

| 测试场景 | SkyTrade Engine | Robyn 0.82 | 倍数 |
|----------|----------------|------------|------|
| GET / (纯文本) | 69,032 req/s | 27,431 req/s | 2.52x |
| GET / 平均延迟 | 3.73ms | 10.31ms | 2.76x |
| GET /hello/bench (JSON) | 64,093 req/s | 29,113 req/s | 2.20x |
| GET /hello/bench 平均延迟 | 4.08ms | 9.27ms | 2.27x |

测试环境：macOS ARM64, Python 3.14, Rust 1.93.1, wrk -t4 -c256 -d10s

---

## 原因一：更轻的 HTTP 栈

| | SkyTrade | Robyn |
|--|----------|-------|
| HTTP 层 | Hyper (纯 Rust，极致精简) | Actix-web (功能完整但更重) |
| 中间件 | 零 | 自带 OpenAPI、docs 路由、日志等 |

Robyn 启动时自动注册额外路由：
```
Added route GET /openapi.json
Added route GET /docs
Docs hosted at http://127.0.0.1:8001/docs
```

这些都是开销。每个请求都要过 Actix 的中间件链。SkyTrade 是裸金属——请求进来直接匹配路由、调 handler、返回。

## 原因二：更少的 GIL 争用

**Robyn 的做法：** 每个请求都经过 Python async 调度（即使 handler 很简单），涉及 Python 协程创建、事件循环调度、GIL 反复获取释放。

**SkyTrade 的做法：**
```rust
// 整个 event loop 在 GIL 之外运行
py.detach(move || {
    rt.block_on(async { /* Tokio 循环 */ })
});

// 只在调 Python handler 的那一瞬间才拿 GIL
Python::attach(|py| {
    handler.call1(py, args)  // 拿 GIL → 调用 → 释放
});
```

GIL 持有时间被压缩到最小粒度——只有 Python handler 执行那几微秒。路由匹配、HTTP 解析、TCP I/O 全在 Rust 层完成，完全不碰 GIL。

## 原因三：更快的路由匹配

| | SkyTrade | Robyn |
|--|----------|-------|
| 路由引擎 | matchit（基于 radix trie，编译时优化） | Actix 内置路由 |
| 复杂度 | O(path_length)，零内存分配 | 更通用但更重 |

matchit 是 Rust 生态中最快的路由库之一，用压缩前缀树匹配，几乎零内存分配。

## 请求生命周期对比

```
Robyn:
  TCP → Actix解析 → 中间件链 → Python async调度 → GIL → 协程创建
  → handler执行 → 协程完成 → GIL释放 → 中间件链返回 → 响应

SkyTrade:
  TCP → Hyper解析 → matchit路由 → GIL → handler执行 → GIL释放 → 响应
```

SkyTrade 砍掉了中间所有不必要的层。

---

## 内存用量对比

测试方式：启动服务器 → 记录空闲 RSS → wrk -t4 -c256 -d10s 压测期间每 0.5s 采样 → 记录峰值和压测后 RSS。

| 指标 | SkyTrade Engine | Robyn 0.82 | 对比 |
|------|----------------|------------|------|
| 空闲 RSS | 10 MB | 35 MB | 3.5x 更省 |
| 峰值 RSS (256并发) | 17 MB | 46 MB | 2.7x 更省 |
| 压测后 RSS | 16 MB | 40 MB | 2.5x 更省 |

SkyTrade 空闲仅 10 MB，满载也只涨到 17 MB。Robyn 光启动就 35 MB（Python 依赖 + Actix + OpenAPI 等组件开销）。这也解释了为什么 SkyTrade 延迟更低——更小的内存占用意味着更好的 CPU 缓存命中率。

---

## Phase 2 可进一步优化的方向

| 优化点 | 当前状态 | 预期提升 |
|--------|---------|---------|
| GIL 批量策略 | 每请求 Python::attach | 减少 GIL 切换开销 |
| 零拷贝响应 | String → Bytes 有拷贝 | 减少内存分配 |
| SIMD-JSON | Python json.dumps | 快 5-10x |
| 多 Worker | 单 Tokio runtime | 利用多核 CPU |
