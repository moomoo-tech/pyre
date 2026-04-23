"""Response caching decorators.

Trade freshness for throughput: stash a handler's JSON-serialized bytes
for a fixed TTL, serve hits straight from cache without re-running the
handler or re-running json.dumps. Designed for public read endpoints
that tolerate brief staleness (``/health``, ``/status``, homepage
leaderboards, public product listings, rate-limited feeds).

Usage::

    from pyronova import Pyronova
    from pyronova.cache import cached_json

    app = Pyronova()

    @app.get("/health")
    @cached_json(ttl=1.0)
    def health(req):
        return {"status": "ok"}

Order matters: ``@cached_json`` must sit **inside** ``@app.get`` so the
framework sees the wrapped function, not the raw handler.

Cache is per sub-interpreter (each TPC worker owns its own dict). With
N workers a hot endpoint does up to N handler evaluations per TTL
window; within the window every subsequent hit is a pure dict lookup.
For cross-worker shared caching back this with ``app.state`` manually —
a few extra Bytes copies, but one miss per TTL across the whole fleet.

Cache key is the request path only. Query strings are ignored. If you
need query-aware caching, pre-compose the key yourself:

    @app.get("/search")
    @cached_json(ttl=5.0, key=lambda req: req.path + "?" + req.query)
    def search(req): ...
"""

from __future__ import annotations

import functools
import json
import threading
import time
from typing import Callable

from .app import Response

__all__ = ["cached_json"]


def cached_json(ttl: float, key: Callable | None = None):
    """Cache a handler's JSON response for ``ttl`` seconds (per worker).

    :param ttl: lifetime in seconds. Must be > 0. Hits older than this
        re-run the handler and replace the cached entry.
    :param key: optional ``f(req) -> str`` to derive the cache key. Default
        keys on ``req.path`` alone.
    """
    if ttl <= 0:
        raise ValueError("cached_json ttl must be > 0")
    key_fn = key if key is not None else (lambda req: req.path)

    def decorator(handler):
        _cache: dict[str, tuple[bytes, float]] = {}
        _lock = threading.Lock()

        @functools.wraps(handler)
        def wrapper(req):
            now = time.monotonic()
            k = key_fn(req)

            entry = _cache.get(k)
            if entry is not None and entry[1] > now:
                return Response(body=entry[0], content_type="application/json")

            result = handler(req)

            # Handler returned an explicit Response — user is signalling
            # a custom status / headers; don't cache, don't rewrap.
            if isinstance(result, Response):
                return result

            if isinstance(result, (bytes, bytearray)):
                body = bytes(result)
            elif isinstance(result, str):
                body = result.encode("utf-8")
            else:
                body = json.dumps(result, separators=(",", ":")).encode("utf-8")

            with _lock:
                _cache[k] = (body, now + ttl)

            return Response(body=body, content_type="application/json")

        return wrapper
    return decorator
