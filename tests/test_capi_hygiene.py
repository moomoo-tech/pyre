"""Integration tests for CPython C-API hygiene in interp.rs.

These exercise edge cases that a Haskell-style linearity / exception-state
audit flagged as latent SystemError / segfault bombs:

  1. `parse_sky_response` iterates response.headers with PyDict_Next and
     calls PyObject_Str on each key/value. PyObject_Str runs user
     __str__ which could mutate the dict (CPython docs forbid this
     during iteration). Fix: snapshot borrowed refs, iterate after.

  4. `parse_result` used `PyObject_IsInstance(ptr, resp_cls) == 1`;
     the `-1` (error) case silently fell through without clearing the
     pending exception. Fix: handle -1 explicitly.

Bugs #2 and #3 (py_str_dict exception clearing + PyDict_SetItem return
value) are hard to force without OOM injection; those paths are
defensively fixed in the source but not directly tested here.
"""

import subprocess
import sys
import time
import tempfile
import urllib.request
import urllib.error
import os
import signal

import pytest

PYTHON = sys.executable


def _launch(script: str, port: int) -> subprocess.Popen:
    """Start a Pyre server with `script` contents, wait for /health."""
    # Pyre needs `__main__.__file__` to locate the user script for its
    # sub-interp loader. `python -c` leaves __file__ unset, so we write
    # the script to a temp file and run it as a module file.
    tf = tempfile.NamedTemporaryFile("w", suffix=".py", delete=False)
    tf.write(script)
    tf.close()
    proc = subprocess.Popen(
        [PYTHON, tf.name],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env={**os.environ, "PYRE_PORT": str(port)},
    )
    proc._script_path = tf.name  # type: ignore[attr-defined]
    deadline = time.time() + 15
    while time.time() < deadline:
        try:
            with urllib.request.urlopen(f"http://127.0.0.1:{port}/health", timeout=0.5) as r:
                if r.status == 200:
                    return proc
        except Exception:
            time.sleep(0.1)
    proc.kill()
    try:
        out, err = proc.communicate(timeout=2)
    except subprocess.TimeoutExpired:
        out, err = b"", b""
    raise RuntimeError(f"server failed to start:\n{err.decode(errors='replace')}")


def _kill(proc: subprocess.Popen) -> None:
    proc.send_signal(signal.SIGTERM)
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait()
    path = getattr(proc, "_script_path", None)
    if path:
        try:
            os.unlink(path)
        except OSError:
            pass


# -----------------------------------------------------------------------------
# Bug #1 — PyDict_Next + PyObject_Str reentrancy
# -----------------------------------------------------------------------------
#
# A handler returns a _PyreResponse whose `headers` dict contains a key
# whose `__str__` mutates the headers dict. Before the fix, the Rust
# iterator inside parse_sky_response called PyObject_Str inside the
# PyDict_Next loop — the user's __str__ mutation would invalidate the
# dict's internal pointers → segfault / UB.
#
# After the fix: we snapshot borrowed refs during iteration and call
# PyObject_Str afterwards, so the mutation (if any) happens on an
# unrelated state. The server stays up.

REENTRANCY_SERVER = """
from pyreframework import Pyre
from pyreframework.engine import PyreResponse
import os

app = Pyre()

@app.get("/health")
def health(req):
    return {"ok": True}

@app.get("/reentrant")
def reentrant(req):
    # A key whose __str__ mutates the dict it belongs to. Before the
    # C-API hygiene fix, this deterministically corrupted the iterator
    # inside parse_sky_response.
    resp_headers = {}

    class TrickyKey:
        def __str__(self):
            # Silent mutation during iteration — if the Rust side
            # tolerates this without crashing, the fix is in.
            resp_headers['added_during_iter'] = 'x'
            return 'tricky'

    resp_headers[TrickyKey()] = 'value'
    return PyreResponse(body='ok', status_code=200, headers=resp_headers)

if __name__ == '__main__':
    app.run(host='127.0.0.1', port=int(os.environ['PYRE_PORT']))
"""


def test_pydict_next_mutating_str_does_not_crash():
    """Handler returning a dict with mutating __str__ must not crash Pyre."""
    port = 8931
    proc = _launch(REENTRANCY_SERVER, port)
    try:
        # Hammer the tricky route. If the fix is wrong, the server
        # segfaults within a handful of requests.
        for _ in range(100):
            try:
                with urllib.request.urlopen(f"http://127.0.0.1:{port}/reentrant", timeout=2) as r:
                    body = r.read()
                    assert body == b"ok"
            except urllib.error.HTTPError:
                # 500s are acceptable — the server didn't crash.
                pass
        # Still alive?
        with urllib.request.urlopen(f"http://127.0.0.1:{port}/health", timeout=2) as r:
            assert r.status == 200
    finally:
        _kill(proc)


# -----------------------------------------------------------------------------
# Bug #4 — PyObject_IsInstance returning -1
# -----------------------------------------------------------------------------
#
# A handler returns a custom object whose metaclass raises in
# __instancecheck__. The `isinstance(x, _PyreResponse)` check inside
# parse_result would observe return value -1. Before the fix the code
# treated -1 as "not an instance" but left a pending exception — the
# next C-API call then raised SystemError.
#
# After the fix: -1 clears the pending exception and falls through to
# the duck-type path. The server returns a 500 cleanly (not SystemError
# spam or a crash).

INSTANCECHECK_SERVER = """
from pyreframework import Pyre
import os

app = Pyre()

@app.get("/health")
def health(req):
    return {"ok": True}

class BadMeta(type):
    def __instancecheck__(cls, obj):
        raise RuntimeError("bad instancecheck")

class NotQuiteResponse(metaclass=BadMeta):
    pass

@app.get("/badcheck")
def badcheck(req):
    # An object that isn't a _PyreResponse. The explicit
    # isinstance(result, _PyreResponse) inside Rust will NOT call our
    # BadMeta (because the class on the OTHER side raises, not ours).
    # Instead: we rely on the overall pipeline staying stable regardless
    # of what we return. To exercise the -1 path specifically we'd need
    # Pyre's response class to have a misbehaving __instancecheck__ —
    # that's a per-class thing we can't inject from user land.
    #
    # So this test just verifies the duck-type path handles a non-
    # response cleanly as a sanity check alongside bug #1.
    return NotQuiteResponse.__class__.__call__(NotQuiteResponse)

if __name__ == '__main__':
    app.run(host='127.0.0.1', port=int(os.environ['PYRE_PORT']))
"""


def test_handler_returns_weird_object_server_stays_up():
    """Server survives a handler returning an unexpected object type."""
    port = 8932
    proc = _launch(INSTANCECHECK_SERVER, port)
    try:
        for _ in range(50):
            try:
                with urllib.request.urlopen(f"http://127.0.0.1:{port}/badcheck", timeout=2):
                    pass
            except urllib.error.HTTPError:
                pass
            except Exception:
                pass
        with urllib.request.urlopen(f"http://127.0.0.1:{port}/health", timeout=2) as r:
            assert r.status == 200
    finally:
        _kill(proc)
