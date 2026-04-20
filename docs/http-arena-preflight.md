# HTTP Arena 赛前分析 — Pyre 提交预测

**Recorded:** 2026-04-19
**Status:** pre-race analysis. Updated when #16-20 land and we have real
           numbers from a staged 64-core run.

## 参考点

| | Actix (current #1) | FastAPI (current Python #1) |
|---|---|---|
| Language | Rust | Python |
| Composite | **818.4** | 117.7 |
| Hardware | AMD Threadripper Pro 3995WX (64C/128T) | same |
| Rank | 1 / 27 | 25 / 27 |

## HTTP Arena 评分方式

Composite = **sum** of per-profile normalized scores (not arithmetic mean).

- 每 profile `rpsScore = (framework_avg_rps / best_avg_rps) × 100`
- 顶框架 per-profile 拿 100 基础分
- 额外最多 **+50 内存效率 bonus**：`sqrt(rps) / memoryMB`
- 15 profiles → 理论天花板 ~2250 (从未有人接近)

## Pyre 当前状态 (v1.5.0)

**Have:**
- Baseline / Plaintext pipelined 路径（8 核 902k/s 已测）
- JSON 响应（serde_json + pythonize，Python dict → Rust JSON 零拷贝）
- Static files serving + O_NOFOLLOW 安全
- Short-lived connections 路径 (sub-interp + hyper)

**Missing** — 5 项 backlog (task #16–#20):
1. Async Postgres (sqlx::PgPool 共享 Arc + 跨 sub-interp channel dispatch)
2. CRUD REST helpers (built on DB)
3. Response compression (gzip / brotli)
4. HTTPS / TLS via rustls
5. Upload streaming (替换 `Limited::new().collect()` buffered 路径)

## 8 核 → 64 核线性外推

| 测试 | 8 核实测 | 纯线性 × 8 | 现实折扣后 (0.7–0.85) |
|---|---|---|---|
| Baseline (`wrk -t4 -c100`) | 423k | 3.4M | **2.5–3M** |
| Pipelined (`wrk -t8 -c256 --pipeline 16`) | 902k | 7.2M | **5–7M** |
| JSON (`/hello/{name}`) | 536k | 4.3M | **3–3.5M** |

折扣来源：NUMA 跨域访问（Threadripper 4 NUMA 域，pyre 目前不做 pinning）+
Tokio scheduler 在 128 threads 的轻微争用 + kernel TCP/epoll 在
百万 req/s 级的 batching 开销。

## 全部实现后的 per-profile 预测

**假设：** #16–#20 全部落地，Pyre v1.6+，64 核 Threadripper 硬件。

| Profile | Pyre 预测 req/s | 归一化 (best→100) | + 内存 bonus | 小计 | 现 Actix 分 |
|---|---|---|---|---|---|
| Baseline | 3M | 100 | +40 | **140** | 150 |
| Pipelined | 7M | 45 (actix 16M 天花板) | +30 | **75** | 140 |
| Short-lived | 500k | 45 | +30 | **75** | 106 |
| **JSON** | **3.5M** | **~290** (反超 actix 1.18M) | **capped at +50** | **150** ✨ | 129 |
| JSON Compressed | 180k | 100 (actix 172k = top) | +35 | **135** | 54 ← **可能反超** |
| JSON TLS | 400k | 76 | +30 | **106** | not participating |
| Upload | 2.5k | 80 | +30 | **110** | 127 |
| Static | 9k | 100 | +30 | **130** | 0.7 (all low) |
| Async DB | 130k | 85 | +35 | **120** | 111 |
| CRUD | 350k | 88 | +30 | **118** | not participating |

**Composite ≈ 1159 (乐观上限)**

## JSON 单项"反超 Actix" 的技术依据

这不是 wishful thinking，是算术：

| | 当前实测 | per core | ratio |
|---|---|---|---|
| Actix JSON | 1.18M @ 64C | **18.4k / core** | (ref) |
| Pyre JSON | 536k @ 8C (`/hello/{name}`) | **67k / core** | **3.6× Actix per-core** |

关键：
- Pyre 的 JSON 响应走的是 `serde_json + pythonize` —— **全 Rust 路径**。Python 只返回 dict，不碰 json.dumps。
- Actix 自然也是全 Rust 路径 (`serde_json`)。两边 Rust JSON 序列化实现一致。
- 我们单核更快的原因：Pyre 响应路径更短（sub-interp worker 直返 Rust response），而 Actix 在 64 核上有 NUMA / scheduler 开销。

风险：HTTP Arena 的 JSON profile 具体形状（固定 payload vs 动态构造）可能和我们现在测的 `/hello/{name}` 不完全对齐。

## 榜单位次区间

| Composite | 位次 | 谁在这档 |
|---|---|---|
| 1200+ | Top 1-2 | 挑战 Actix 818 / aspnet-minimal 775 |
| 900-1100 | Top 3-5 | 超越 aspnet, 紧逼 Actix |
| **600-800** ← **现实中值** | **Top 5-10** | Swoole 740 / workerman 729 一档 |
| 400-600 | Top 10-15 | 超越 elysia / aspnet-mvc |

**保守预测:** composite **~600-700** → **Top 8-10, Python #1 by 5×**
**中值预测:** composite **~800-1000** → **Top 3-5**
**乐观上限:** composite **~1100+** → **挑战 Actix 榜首**

## 关键前提 / 风险

1. **JSON 反超** 依赖 HTTP Arena 的 JSON profile 和我们 `/hello/{name}` 的
   shape 类似。若测试用固定 payload（如 `{"message": "Hello, World!"}`），
   我们可以预先构造 bytes 返回，进一步加速。如果测试要求真实 JSON 序列化
   不能 cache response，则优势缩小到 2×，仍胜。
2. **内存 bonus** 基本稳了 —— v1.5.0 实测 0.057 B/req（上限测量噪声），
   远低于多数 Python 框架。memory-efficiency bonus 公式
   `sqrt(rps)/memoryMB` 给 Pyre 好分。
3. **Sub-interp 64 并发创建** 没有在 64 核硬件上验证过 —— CPython 全局
   锁在 interp 创建阶段是否串行化未知。**若 16 interp 线性但 64 interp
   卡主 → 启动时间长、稳态 RPS 不受影响**。
4. **DB profile 竞争** 实际看 sqlx pool size 调优和 Postgres 本身响应
   延迟 —— 不全在 Pyre 代码控制内。
5. **Upload streaming** 落地难度比其他 4 项都大，涉及 Python 侧 API 变化
   (`async for chunk in req.stream()`)。

## 最保守假设

**即便以上全部打 6 折**，Pyre 依然：
- Composite ≥ 500 → Top 12 档
- Python 全场第一，碾压 FastAPI 117.7（4–5× 分差）
- 全语言挤进 Top 15（当前 Top 15 = aspnet-mvc 375）

## 执行序

建议最优先 → 最低优先：
1. **#16 DB (sqlx::PgPool)** —— 解锁 Async DB + CRUD 两个 profile (~240 分)
2. **#17 CRUD** —— #16 之上小量工作
3. **#18 Compression** —— 独立，低风险，+135
4. **#20 Upload streaming** —— 工作量中等，+110
5. **#19 TLS (rustls)** —— 仅为 HTTP/2 profile 做铺垫，+106

前 3 项做完就能冲 Top 5。5 项全做冲 Top 3。

---

本文档是**赛前战术分析**，不是承诺。实测数据落地后会以 post-race
diff 形式更新。
