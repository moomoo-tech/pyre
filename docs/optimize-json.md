# Optimize JSON

Plan to lift the JSON serialization throughput on Pyronova's response path.

## Current state

Pipeline in `src/response.rs`:

```
Python obj  →  py_to_json_value (src/json.rs)  →  serde_json::Value
            →  sonic_rs::to_vec(&val)          →  Vec<u8>  →  Bytes
```

Two passes over the data, plus a full `serde_json::Value` tree as an
intermediate. Every key string is cloned (`to_string_lossy().into_owned()`).
Every sub-object is a `serde_json::Map<String, Value>` allocation.

Score on the TechEmpower-style suite: **JSON 30.8 (329k req/s)**, JSON
Compressed 21.0, JSON TLS 47.3.

### Why it's like this

The current `JsonContext` (commit `3b350a2`) was rewritten with correctness as
the primary goal: JSON Path errors, circular reference detection, depth limit
(256), surrogate handling, bigint preservation, bool/int subclass isolation.
The intermediate `serde_json::Value` tree made those guarantees easy to
implement. Throughput was not the bottleneck the framework was being benched
on — sub-interp + channel pool dominated the wins.

TechEmpower's JSON test changes the ruler: the payload is tiny
(`{"message":"Hello, World!"}`), so framework overhead is fully exposed and
each redundant allocation costs.

## Optimization roadmap (ranked by ROI)

### 1. Direct Python → bytes serializer (biggest win)

Replace `py_to_json_value` returning `serde_json::Value` with a writer that
emits bytes directly:

```rust
pub(crate) fn py_to_json_bytes(
    obj: &Bound<'_, PyAny>,
    out: &mut Vec<u8>,
) -> Result<(), PyJsonError>;
```

- `PyString` → `to_str()?` borrows the UTF-8 buffer; escape directly into `out`
- `PyInt` → `itoa::write(out, v)`
- `PyFloat` → `ryu::write(out, v)` (handle NaN/Inf as today)
- `PyDict`/`PyList`/`PyTuple` → write `{` `,` `}` / `[` `,` `]` inline

Eliminates the entire `serde_json::Value` tree and every key `String` clone.
Path tracking, circular detection, depth limit, signal checks all stay —
they're orthogonal to the output representation.

Expected: **+30–50%** on JSON throughput. Even larger relative win on big
payloads (the intermediate tree scales linearly with payload size).

### 2. SIMD-accelerated string escape

Most strings contain no characters that need escaping. Fast path:

```rust
// memchr3(b'"', b'\\', 0x00..0x1F) → if no hit, extend_from_slice whole span
```

Fall back to per-byte escape only when a control/quote/backslash is found.
This is what `sonic_rs` does internally, but the intermediate tree currently
re-encodes the bytes on the second pass. Once #1 lands, this becomes the
hot loop.

### 3. Thread-local output buffer reuse

Each request currently allocates a fresh `Vec<u8>` and grows it through
doubling. Reuse a per-worker buffer:

```rust
thread_local! {
    static JSON_BUF: RefCell<Vec<u8>> = RefCell::new(Vec::with_capacity(4096));
}
```

Clear before serialize, copy into `Bytes` after. Saves the realloc chain and
the initial small allocations on hot paths.

### 4. Dict key fast path

`coerce_dict_key` currently does `to_string_lossy().into_owned()` for every
`PyString` key — one clone per key. After #1 we can borrow `to_str()?` and
escape directly into the output buffer, zero allocation per key.

### 5. Cheaper signal checking

`SIGNAL_CHECK_INTERVAL = 1000` is already sparse, but `element_count += 1`
+ modulo runs per element. For payloads under 1000 elements (the vast
majority of API responses), the check never fires and the bookkeeping is pure
overhead. Options:

- Skip the increment entirely when total element count is bounded by the
  initial dict/list size hint
- Increase to 4096 — the user-facing latency of a non-cancellable
  serialization is still sub-millisecond

### 6. Compression tuning (separate axis)

JSON Compressed score is 21.0 — likely re-initializing the encoder per
request and/or using a high compression level. To investigate:

- Reuse a `flate2`/`brotli` encoder across requests (per-worker)
- For brotli, level 4–5 is the throughput/ratio sweet spot; 11 is a trap
- Consider streaming the encoder into the response buffer instead of
  `compress(full_body)` after serialization

## Out of scope (separate phase)

### Large-payload GIL holding

`py_to_json_value` walks the whole Python tree under the GIL. A 100MB dict
response stalls the sub-interp for tens of milliseconds — bad for tail
latency. Two possible fixes, both invasive:

- Deep-copy the Python tree into an `Arc<JsonNode>` Rust representation,
  then `py.detach()` and serialize without the GIL
- Periodically `detach`/`attach` mid-walk (every N nodes) — complicates
  circular detection and path tracking

Defer until we see real workloads with >1MB JSON responses.

### simd-json / sonic-rs full integration

Once #1 lands we're emitting bytes directly, so `sonic_rs::to_vec` is gone
from the path. There may still be a win from using sonic_rs's number
formatter or escape routine as building blocks — evaluate after #1.

## Validation

Before/after benchmarks to run on each step:

1. `benchmarks/run_comparison.sh` — Pyronova vs FastAPI
2. TechEmpower JSON / JSON Compressed / JSON TLS subscores
3. A new microbench: 1KB / 100KB / 10MB JSON payloads, throughput + p99
4. `pytest tests/` — all 65 JSON correctness tests must pass unchanged

## Suggested execution order

1. Land #1 + #4 together (single PR, share most of the new writer code)
2. Measure, then add #2 if escape shows up in profile
3. Add #3 (cheap, isolated change)
4. #5 only if profile still shows signal-check overhead
5. #6 as a separate PR — touches the response pipeline, not the serializer
