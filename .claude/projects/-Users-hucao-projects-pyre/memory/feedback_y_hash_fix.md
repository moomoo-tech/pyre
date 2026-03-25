---
name: Phase 7.2 FFI format string fix
description: When implementing pyre_send_cfunc, use y# (not z#) for Python bytes args. z# is for str/None only.
type: feedback
---

When implementing Phase 7.2 async bridge (`pyre_send_cfunc` in interp.rs):
- Use `c"nKHsy#"` format string for `PyArg_ParseTuple` (not `z#z#`)
- `y#` = bytes, `z#` = str/None, `s` = str
- Don't pass `&mut body_len` twice (pointer overwrite bug)
- Always `PyErr_Print()` on parse failure, never silently return null
- Consider replacing global `Mutex<HashMap>` with `OnceLock<Vec<WorkerState>>` for zero-lock at 200k+ QPS

**Why:** Python side sends `json.dumps(res).encode('utf-8')` which is `bytes`, not `str`. Wrong format causes silent parse failure → tx.send never fires → request hangs → Tokio pool exhaustion.

**How to apply:** When writing `pyre_send_cfunc` for Phase 7.2 implementation.
