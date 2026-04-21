"""Kubernetes-style health probes — ``/livez`` + ``/readyz``.

Wire-up::

    from pyreframework import Pyre
    from pyreframework.db import PgPool

    app = Pyre()
    app.enable_health_probes()   # /livez + /readyz auto-registered

    pool = PgPool.connect(...)

    @app.readiness_check("db")
    def _db_ready():
        pool.fetch_scalar("SELECT 1")         # raises on failure

    @app.readiness_check("cache")
    async def _cache_ready():
        await redis.ping()

Behaviour:

- ``GET /livez`` always returns ``200 {"status":"alive"}``. The process
  is running; that's all this probe answers. k8s uses it to decide
  whether to restart the pod.
- ``GET /readyz`` runs every registered check. Success → ``200
  {"status":"ready","checks":{...}}``. Any failure (exception or
  falsy-non-None return) → ``503 {"status":"not_ready","checks":{...}}``.
  k8s uses this to gate traffic.

Checks run sequentially in the handler. Keep them fast — a readyz
handler is a hot loop during rolling deploys. Sync + async both work;
async checks are awaited from the async pool.
"""

from __future__ import annotations

import asyncio
import json
import inspect
from typing import Any, Awaitable, Callable, Union

from pyreframework.engine import PyreResponse


CheckFn = Union[Callable[[], Any], Callable[[], Awaitable[Any]]]


def _run_checks_sync(checks: list[tuple[str, CheckFn]]) -> tuple[bool, dict[str, Any]]:
    """Run every check, catching exceptions. Returns (all_ok, results)."""
    results: dict[str, Any] = {}
    all_ok = True
    for name, fn in checks:
        try:
            if inspect.iscoroutinefunction(fn):
                # Drive the coroutine on a private loop — readyz is a
                # cold-path call, the throwaway loop is fine.
                res = asyncio.new_event_loop().run_until_complete(fn())
            else:
                res = fn()
            if res is False:
                results[name] = {"ok": False, "error": "check returned False"}
                all_ok = False
            else:
                results[name] = {"ok": True}
        except Exception as e:  # noqa: BLE001 — probe must never crash
            results[name] = {"ok": False, "error": f"{type(e).__name__}: {e}"}
            all_ok = False
    return all_ok, results


def _build_livez_handler():
    body = json.dumps({"status": "alive"}).encode("utf-8")

    def livez(req):
        return PyreResponse(body=body, content_type="application/json")

    return livez


def _build_readyz_handler(checks: list[tuple[str, CheckFn]]):
    def readyz(req):
        ok, results = _run_checks_sync(checks)
        payload = json.dumps({
            "status": "ready" if ok else "not_ready",
            "checks": results,
        })
        return PyreResponse(
            body=payload,
            status_code=200 if ok else 503,
            content_type="application/json",
        )

    return readyz


__all__ = ["CheckFn"]
