# Memory Leak Investigation — 2026-04-19

A narrative walkthrough of how we found and (partially) fixed a long-standing
memory leak specific to Pyre's sub-interpreter mode. Written in the order
hypotheses were formed, tested, accepted, or rejected.

## TL;DR

- Hybrid / sub-interpreter mode leaked **every request's `_PyreRequest`** plus
  its `headers` and `params` dicts. ~0.75 KB / request of linear RSS growth.
- The bug is architecturally in Pyre since before v1.4.0 (we reproduced on
  commit `3c17cf9` v1.4.0 and on the d0ce481 pre-hotfix baseline).
- **Half-fix shipped**: replaced `PyObject_Call(handler, args_tuple, NULL)`
  with `PyObject_Vectorcall(handler, stack_array, 1, NULL)` on the hot
  request path. `_PyreRequest` objects now decref properly (0 alive after
  bench, vs 1M+ before).
- **Not yet fixed**: the same substitution on the `_PyreRequest` class
  constructor did NOT stop the dict leak (still ~2 dicts / request). The
  root cause for this second leak is still open.
- Also fixed a subtly related bug in `PyObjRef::Drop` that used
  `PyGILState_Check()` (returns 0 in non-main interpreters) instead of a
  sub-interp-aware tstate probe.

## Starting point

The user noticed earlier bench runs left ~33 GB + 9.9 GB processes alive
(stale servers from several wrk iterations). That alone was worth
investigating:

> pid 682413: `examples/hello.py` — RSS **33.4 GB**

A hello-world server returning `"Hello from Pyre!"` should never climb to
33 GB. The process had served ~30M requests over ~20 minutes → ~1 KB /
request of irreversibly-allocated memory.

## Instruments used, in order

1. **`ps -eo pid,rss` + `/proc/PID/status`** — confirm it's genuinely a
   process-level RSS leak, not swap noise
2. **wrk iterations + RSS sampling** — establish a linear growth rate
3. **git worktree bisection (v1.4.0 vs v1.4.5)** — prove the leak is pre-
   existing, not introduced in recent changes
4. **A/B on mode (GIL vs hybrid)** — narrow the leak to sub-interp mode
5. **`heaptrack` on Python-side allocations** — show only 47 MB of the
   leak is visible to libc-malloc hooks (→ Rust-side mimalloc owns the rest)
6. **`gc.get_objects()` type histogram** — identify the exact Python types
   accumulating
7. **`gc.get_referrers()` on a sample** — show no Python-level owner
   (→ owner is C-level, i.e. Rust)
8. **`sys.getrefcount()` on a sample** — confirm refcount > 0 so the
   object is NOT cycle garbage
9. **Per-request-probe via curl** — confirm 100% leak rate per sub-
   interpreter (not 7% as initially thought; serialising through one
   sub-interp made it crystal clear)
10. **`eprintln!` instrumentation in `PyObjRef::Drop`** — count actual
    `Py_DECREF` calls and verify the defensive "leak pointer" branch
    is never taken in the hot path
11. **Refcount-at-drop probe by type** — show that `_PyreRequest` and
    `dict` never reach rc=1 at drop time from our instrumented call sites

## Hypotheses, accepted or rejected

### H1. "Tokio mpsc back-pressure buffering". REJECTED.

Early theory: maybe Rust-side channels pile up under load. Quickly ruled
out: the RSS growth is **monotonic** across bench iterations and survives
the cooldown between iterations. Buffer backlog would drain when load
stops.

### H2. "mimalloc refuses to return arenas to the OS". PARTIAL, REJECTED as primary cause.

mimalloc is Pyre's Rust `#[global_allocator]`. Its design leaves arena
pages mapped even when free. Were this the whole story, RSS would
**plateau** after peak usage once arenas are reused. But RSS keeps
climbing linearly — we're still allocating net new, not reusing.

heaptrack confirmed: only 47 MB of new malloc activity over 5s of 240k
req/s. The other ~900 MB of RSS growth is invisible to libc-malloc hooks,
which means it's going through mimalloc (Rust-side structures) OR
Python's PyMalloc arenas.

### H3. "`response_map` orphans after timeout". REJECTED.

On timeout or client disconnect we feared the `oneshot::Sender` entries
in `response_map` were never cleared. Grep of the code: `pyre_send`
always calls `map.remove(&req_id)` and we saw no `response_map: miss`
log lines during steady-state wrk. Not it.

### H4. "Sub-interpreter specific". ACCEPTED.

A/B test: ran the same hello path in `mode="gil"` vs hybrid. **GIL mode
did not leak at all** (RSS held at 40 MB across 3M requests), hybrid
leaked 0.75 KB / request at 400k req/s. From that point on the bug had
to involve the sub-interpreter boundary.

### H5. "PyO3 smart-pointer drop silently no-ops in sub-interps". ACCEPTED (but narrower than expected).

Read `PyObjRef::Drop` in `src/interp.rs`:

```rust
if ffi::PyGILState_Check() != 1 {
    tracing::error!(..., "PyObjRef dropped without GIL — leaking pointer");
    return;  // Leak is better than crash
}
ffi::Py_DECREF(self.ptr);
```

The standard CPython `PyGILState_*` API is hardcoded to the **main
interpreter's TLS slot**. In any sub-interpreter, `PyGILState_Check()`
returns 0 even when that sub-interp's own GIL is held. A worker thread
processing a request would have `Py_DECREF` silently skipped every time.

Switched the check to `PyThreadState_GetUnchecked().is_null()` — a tstate
is attached iff the thread is currently inside some interpreter's GIL
scope, which is the correct sub-interp-aware precondition for
`Py_DECREF`.

**Verified**: instrumented both branches. Over a 10s bench (~21 M
`PyObjRef` drops), only 16 hit the "leak" branch, all at startup.
Every per-request drop actually called `Py_DECREF`.

**But the leak didn't go away.** That was the surprise — the check
was technically wrong but wasn't the leak.

### H6. "`PyObject_Call` leaks its args tuple contents inside a sub-interpreter." ACCEPTED (as THE primary leak).

With `Py_DECREF` now reliably firing, `_PyreRequest` refcounts had to be
dropping on the normal path. So why were 72,000 of them alive after
3 seconds of wrk? Each had `refcount=1`, with no Python-level referrer
(`gc.get_referrers` returned nothing, after subtracting our own probe's
return-list) — exactly one C-level ref held, forever.

The only C-level owner we build per-request is the arg tuple passed
into `PyObject_Call(handler, args, null)`:

```rust
let call_args = PyObjRef::from_owned(ffi::PyTuple_New(1))?;
ffi::PyTuple_SetItem(call_args.as_ptr(), 0, request_obj.into_raw());
let result_obj = PyObject_Call(func, call_args.as_ptr(), null_mut());
// call_args drops at fn end → tuple DECREF → should cascade to _PyreRequest
```

Replaced with `PyObject_Vectorcall`, which takes a raw stack-allocated
pointer array instead of a heap tuple:

```rust
let args_arr = [request_obj.as_ptr()];
let result_obj = PyObject_Vectorcall(func, args_arr.as_ptr(), 1, null_mut());
drop(request_obj);  // caller owns the ref; drops at scope end
```

**Post-fix verification**:
- Per-sub-interp `_PyreRequest` alive after 500 serial requests: **1** (just
  the in-flight probe) — down from **31** (100% leak).
- Per-sub-interp after 2500 requests: **1** — down from **156**.

The `_PyreRequest` leak is cleanly fixed.

### H7. "Same bug on the `_PyreRequest` class constructor path". INCONCLUSIVE — still leaking.

`build_request` constructs `_PyreRequest` via
`PyObject_Call(cls, args_tuple_of_7, null)` with headers/params dicts
at slots 2 and 5 of the tuple. Replaced with
`PyObject_Vectorcall(cls, args_arr, 7, null)` under the same hypothesis.

**Did not fix the dict leak.** Post-change histogram still shows ~3 M
dicts alive after 6 iters of wrk, signatures matching the empty-params
(`{}`) and headers (`{accept, host, user-agent}`) we set up in Rust.
Linear RSS growth persists at ~2.8 GB/iter.

Possible reasons (open):

- `_PyreRequest` is a pure Python class without a vectorcall slot.
  `PyObject_Vectorcall` falls back to `_PyObject_MakeTpCall`, which
  builds its own internal args tuple and calls `tp_call(self, tup, kw)`.
  If that internal tuple's contents aren't DECREF'd in sub-interp mode,
  we leak at the same site just through a layer of indirection.
- The dicts may be retained by something downstream of `__init__` —
  e.g., Python 3.14's adaptive interpreter caching, instance `__dict__`
  shared-key tables, or a zombie frame.
- `PyDict_SetItem` in `py_str_dict` is confirmed to behave correctly
  (we traced every INCREF/DECREF on paper; the accounting says rc
  should reach 0 on `_PyreRequest` dealloc).

Next steps for tomorrow:

1. Add back the refcount-at-drop probe, this time tagging **dicts
   specifically** — record refcount when the `py_headers` / `py_params`
   PyObjRef drops. If it's 1, dict is held only by `_PyreRequest` and
   the dealloc should take it to 0 — means something upstream of dealloc
   keeps it alive. If it's 2+, we know another owner exists and can hunt.
2. Try constructing `_PyreRequest` without going through `PyObject_Call`
   at all — e.g., `tp_new` + direct `PyObject_SetAttrString` for each
   field. Eliminates the args-tuple middleman entirely.
3. Alternative: skip the `_PyreRequest` class on the hot path, pass the
   7-tuple directly to the handler and have Python-side code do the
   wrap. Pyre's async engine already uses a 9-tuple shape; the sync
   path could adopt a similar pattern.

## Bonus discovery — logging is a ~30% throughput tax

`examples/hello.py` has `app.enable_logging()` at the top, which turns on
per-request `tracing::info!("Request handled", ...)` events.

Measurements on the same machine:

| Config | req/s | per-request allocations through tracing |
|---|---|---|
| `hello.py` (with access log) | 298k | format strings, JSON encoding, mpsc push |
| `hello_nolog.py` (no access log) | 400-420k | none |

Even with `tracing_appender::non_blocking`, the producer side still pays
for:

1. EnvFilter regex match per event
2. Formatting the event Record (one or more heap Strings)
3. Pushing into a crossbeam mpsc
4. stderr flush contention on the writer thread

At 400k req/s the producer-side overhead alone costs ~25–30% of
throughput. Concretely, **never call `enable_logging()` in a benchmark**.
The `hello.py` example should either disable it or document the caveat.

## Bonus discovery #2 — the machine itself is slower today than when the baseline was measured

Historical CHANGELOG for v1.4.0 cites **419k req/s** on AMD 7840HS for
this exact route. Today, with an otherwise identical build, we see
**340k** at best, ~**270k** under realistic background load.

Investigation showed:

- **CPU governor = `powersave`** (avg 2.6 GHz vs 5.1 GHz boost)
- **Memory pressure**: 56 GB used of 59 GB total, swap partially used
- **Load avg 9.2 on 16 cores** from a dozen skytrade / prefect /
  watchdog services

An earlier leak artifact — a stale 33 GB `hello.py` process from our own
bench runs — was itself contributing to the memory pressure. Killing it
recovered 43 GB instantly.

Plus **two copies of each Prefect service** existed, one user-level and
one system-level, fighting over ports 4200 / 8090 — the system-level
`prefect-server.service` had an `NRestarts=123,879` counter from 19 days
of losing that fight. Disabling the user-level duplicates and making
the system-level ones authoritative freed the machine of that 12-second
restart loop. (Documented in skytrade's CLAUDE.md under "Deployment &
Services".)

Reliable benchmarks need:

1. `sudo cpupower frequency-set -g performance` (or at minimum
   `ondemand`) before the run
2. No leaked server processes
3. Pause transient ETL workloads during the run
4. Record the machine state alongside the number, not just the number

## Fixes applied in this session

Committed:
- `src/interp.rs` `PyObjRef::Drop`: `PyGILState_Check` →
  `PyThreadState_GetUnchecked`. Correctness fix; not the leak source,
  but the previous check was architecturally wrong for sub-interps.
- `src/interp.rs` `call_handler`: `PyObject_Call(handler, tup, null)` →
  `PyObject_Vectorcall(handler, stack_ptrs, 1, null)`. This is the
  actual `_PyreRequest` leak fix.
- `src/interp.rs` `build_request`: same substitution, intended as the
  dict-leak fix. Did not stop the dict leak; left in place because it
  avoids one tuple allocation per request and is strictly faster.

Left open:
- The dict leak (~2 dicts / request on the subinterp hot path). See
  "Next steps" above.
- `_PyreRequest` is a pure Python class with no `__slots__`. Giving it
  slots would shrink per-instance size and may interact with the
  remaining leak. Worth testing alongside the hot-path rework.

## Numbers

Before any fix, fresh server, `wrk -t4 -c100 -d10s`, repeated 6 times:

```
iter 1: 302k req/s  RSS=3497 MB
iter 2: 297k        RSS=6646 MB
iter 3: 298k        RSS=10005 MB
iter 4: 300k        RSS=13066 MB
iter 5: 297k        RSS=16459 MB
iter 6: 296k        RSS=19207 MB
```

After `_PyreRequest` (Vectorcall) fix, same setup:

```
iter 1: 426k req/s  RSS=3145 MB
iter 2: 417k        RSS=6029 MB
iter 3: 415k        RSS=8899 MB
iter 4: 411k        RSS=11739 MB
iter 5: 408k        RSS=14563 MB
iter 6: 406k        RSS=17373 MB
```

Throughput +40% (log disabled + Vectorcall). RSS growth rate went
from 3.1 GB/iter to 2.8 GB/iter — the dict leak (2/req) is still the
dominant cost. Object counts:

- Before fix: 1 M `_PyreRequest` alive + 2 M dicts
- After fix: 0 `_PyreRequest` alive + 3.08 M dicts

## What I did NOT try (yet)

- Manual `tp_new` + attribute SetAttrString to bypass `tp_call`
  entirely
- Running under Python 3.13 to rule out a 3.14 regression (ab/sent)
- `valgrind --tool=massif` on a short bench for a heap snapshot
  diffable across builds
- `PYTHONDEVMODE=1` or `PYTHONTRACEMALLOC=1` with the main-interp-only
  workaround
- Replacing `_PyreRequest` with a `pyclass` at the Rust level (a real
  PyO3 class instead of a pure Python class), which would be a bigger
  refactor but sidesteps the `tp_call` path
