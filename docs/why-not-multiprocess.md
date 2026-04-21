# 为什么 Pyronova 不做多进程模式

> 技术决策文档 — 2026-03-24

## 背景

压测显示 Robyn `--fast`（22 进程）在 numpy 场景下领先 Pyronova 3.7x：

| 场景 | Pyronova Hybrid | Robyn --fast | 差距 |
|------|-----------|-------------|------|
| numpy mean(10k) | 8,507 | 31,261 | 3.7x |
| numpy SVD 100x100 | 3,993 | 5,079 | 1.3x |

看似应该加多进程追平。**但我们不做。**

## 物理原因：为什么 Robyn numpy 快

```
Robyn --fast = 22 个 OS 进程 × 22 个独立解释器 × 22 个 GIL = 多核全开
Pyronova gil=True = 1 个进程 × 1 个主解释器 × 1 个 GIL = 单核瓶颈
```

numpy 的 `_multiarray_umath` C 模块主动拒绝在子解释器中加载（`ImportError: cannot load module more than once per process`）。所以 Pyronova 的 numpy 路由只能走主解释器，单线程串行。

## 为什么不加多进程

### 理由一：摧毁 Pyronova 的核心优势

Pyronova 的杀手锏是**单进程 + 多子解释器**：

| 指标 | Pyronova (1 进程) | Robyn --fast (22 进程) |
|------|-------------|---------------------|
| 内存 | ~67 MB | ~451 MB |
| 进程间通信 | 无（共享内存） | IPC 开销 |
| 状态共享 | 天然共享 | 需要 Redis/共享内存 |
| 部署复杂度 | 1 个进程 | 22 个进程协调 |

加多进程 = 变成又一个 Gunicorn 变体，丢掉内存 7x 优势。

### 理由二：99% 的 Web 请求是 I/O，不是 CPU

真实后端负载分布：

```
I/O 密集 (数据库/API/缓存/文件)  ████████████████████░  95%
轻 CPU (JSON 序列化/校验/路由)    ███░                   4%
重 CPU (numpy/pandas/ML 推理)     █░                     1%
```

为 1% 的场景引入多进程架构复杂度，代价不值得。

### 理由三：AI 应用 ≠ CPU 密集

AI Agent 后端的真实调用链：

```python
@app.get("/chat")
async def chat(req):
    prompt = req.json()["prompt"]

    # 1. 向量检索 (网络 I/O) — 等 20ms
    docs = await vector_db.search(prompt)

    # 2. 调用 LLM (网络 I/O) — 等 2000ms
    response = await openai.chat(prompt, context=docs)

    # 3. 存日志 (网络 I/O) — 等 5ms
    await db.insert_log(prompt, response)

    return {"reply": response}
```

全链路 2025ms，其中 CPU 时间 < 1ms。**瓶颈 100% 在 I/O 并发。**

### 理由四：量化交易也是 I/O 为主

SkyTrade 交易系统的实际架构分层：

```
┌─────────────────────────────────────────────────┐
│ Pyronova Web 层 (async I/O)                         │
│  · WebSocket 行情接收          ← 纯 I/O         │
│  · REST API 订单提交           ← 纯 I/O         │
│  · 鉴权/风控校验               ← 轻 CPU         │
└──────────────────┬──────────────────────────────┘
                   │ 内部消息队列
┌──────────────────▼──────────────────────────────┐
│ 计算层 (独立进程/Celery/C++ 引擎)               │
│  · Pandas 回测                 ← 重 CPU，不在 Web 层 │
│  · 策略引擎                    ← 重 CPU，不在 Web 层 │
│  · 风险模型                    ← 重 CPU，不在 Web 层 │
└─────────────────────────────────────────────────┘
```

在 Web Handler 里做 `pandas.groupby().rolling()` 是**反模式（Anti-pattern）**。
重计算应该在后台进程中完成，Web 层只负责接收和分发。

### 理由五：Polars 才是正确答案

面对金融数据处理，应该引导用户从 Pandas 迁移到 **Polars**：

| 特性 | Pandas | Polars |
|------|--------|--------|
| 底层语言 | C + Python | **Rust** |
| 多线程 | ❌ 单线程 | ✅ 自动多核 |
| GIL 行为 | **持有 GIL** | **释放 GIL** |
| 在 Pyronova gil=True 中 | 阻塞主解释器 | 不阻塞，Pyronova 可继续接请求 |

用 Polars 在 `gil=True` 路由中做 2 秒的数据聚合，主 GIL 会被释放，
Pyronova 仍然可以疯狂处理其他请求。完美共存。

### 理由六：numpy 的问题是 numpy 的，不是 Pyronova 的

numpy 不支持子解释器（PEP 684），这是 numpy 的技术债：

- CPython tracking: PEP 734 (Python 3.14 stdlib interpreters)
- numpy tracking: [numpy#24003](https://github.com/numpy/numpy/issues/24003)

随着 Python 3.14/3.15 推进，C 扩展生态会逐步适配多阶段初始化。
届时 numpy 可以直接在 Pyronova 子解释器中运行（10 个独立 GIL 并行），
吞吐量会超过 Robyn 的多进程方案（更少开销，零 IPC）。

## 正确的投资方向：async handler

解决 Robyn 唯一真正的领先场景：

```
sleep(1ms) I/O 模拟:
  Robyn    77,967 req/s
  Pyronova      7,905 req/s  ← 10x 落后，因为 sync handler 阻塞 worker
```

实现 async handler 后，Pyronova 的 I/O 并发能力将与 Robyn 持平甚至超越，
同时保持子解释器模式的 CPU 并行优势和内存优势。

**最终目标：I/O + CPU + 内存 三杀 Robyn。**

## 结论

| 方案 | 收益 | 代价 | 决策 |
|------|------|------|------|
| 多进程 numpy | numpy 场景追平 Robyn | 内存 7x，架构复杂度，丢掉核心优势 | **❌ 不做** |
| async handler | I/O 场景追平/超越 Robyn | 实现复杂度中等 | **✅ 立即做** |
| 等 numpy 适配 PEP 684 | numpy 自动在子解释器中并行 | 等待上游 | **⏳ 长期** |
| 推荐 Polars 替代 Pandas | 重计算不阻塞 GIL | 文档工作 | **✅ 文档引导** |
