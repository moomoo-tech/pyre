# Python GC 优化指南 — 金融量化场景

> 适用于 Pyre 框架下的高频交易、实时行情、多因子计算等场景

## 为什么 GC 在金融场景下致命

Python 的 GC (Garbage Collector) 执行时会 Stop-The-World：
- Gen0 回收：~0.1ms（通常无感）
- Gen1 回收：~1-5ms
- Gen2 回收：~10-50ms（**致命：足以错过一个交易窗口**）

### Pyre 架构下的影响

| 模式 | GC 影响 | 检测方式 |
|------|---------|---------|
| GIL 模式 | GC 冻结整个主解释器 | Watchdog 延迟飙升 |
| Sub-interp 模式 | GC 只冻结当前 worker | Event Loop lag 监控 |
| Hybrid 模式 | GIL 路由受 GC 影响，sub-interp 路由不受 | 两个指标结合分析 |

### 如何用 Watchdog 检测 GC 停顿

现象特征（`/__pyre__/metrics` 看板）：
```json
{
    "gil_peak_us": 45000,    // 45ms 延迟毛刺 ← GC 嫌疑
    "memory_rss_mb": 120.5,  // 内存稳定（非泄漏）
    "cpu_usage": 0.3         // CPU 未打满（非计算瓶颈）
}
```

如果 GIL peak 频繁出现 10-50ms 毛刺，CPU 不满，内存震荡 — 90% 是 GC。

## 框架层面的优化策略

### 策略 1：手动 GC 控制（推荐）

```python
import gc

# 在高频交易时段禁用自动 GC
gc.disable()

@app.get("/trade", gil=True)
def handle_trade(req):
    # 处理交易信号，零 GC 风险
    return execute_trade(req.json())

# 在闲置时段手动回收
@app.get("/gc", gil=True)
def manual_gc(req):
    collected = gc.collect(generation=0)  # 只回收年轻代
    return {"collected": collected}
```

### 策略 2：消灭隐式对象分配

```python
# ❌ 坏：每行产生临时数组，触发 GC
result = prices * weights + bias
normalized = (result - mean) / std

# ✅ 好：就地计算，零分配
np.multiply(prices, weights, out=buffer1)
np.add(buffer1, bias, out=buffer1)
np.subtract(buffer1, mean, out=buffer2)
np.divide(buffer2, std, out=result)
```

### 策略 3：Zero-copy 数据传递

```python
@app.get("/quotes", gil=True)
def get_quotes(req):
    # Rust 传来的 bytes 直接转 numpy，零 Python 对象分配
    raw_bytes = req.body
    prices = np.frombuffer(raw_bytes, dtype=np.float64)
    # prices 直接指向 Rust 内存，GC 无感知
    return {"mean": float(prices.mean())}
```

### 策略 4：对象池（适合高频重复计算）

```python
# 预分配固定大小的 numpy 数组，反复使用
class QuoteBuffer:
    def __init__(self, size=1000):
        self.prices = np.zeros(size)
        self.volumes = np.zeros(size)
        self.signals = np.zeros(size)

    def update(self, raw_data):
        # 就地更新，不分配新对象
        np.copyto(self.prices, raw_data[:1000])

# 全局单例（通过 app.state 跨 worker 共享元数据）
buffer = QuoteBuffer()
```

## Pyre 框架未来计划

| 功能 | 状态 | 说明 |
|------|------|------|
| GIL Watchdog 延迟探测 | ✅ 已实现 | `PYRE_METRICS=1` 启用 |
| 内存 RSS 监控 | ✅ 已实现 | `get_gil_metrics()` 返回 RSS |
| Event Loop lag 监控 | 📋 Phase 7.2 | asyncio 心跳协程 |
| `app.gc_control()` API | 📋 计划 | 框架层面的 GC 开关 |
| Zero-copy `memoryview` 传递 | 📋 计划 | Rust→Python 数据无拷贝 |
| 预分配缓冲池 API | 📋 计划 | `app.buffer_pool(size, dtype)` |
