"""Async Postgres support for Pyronova handlers.

Thin Python-side re-export of the Rust `PgPool` class. Initialize once at
startup, then call `pool.fetch_one(...)`, `.fetch_all(...)`, `.fetch_scalar(...)`,
`.execute(...)` from any handler — sub-interpreter routes work natively
via the C-FFI DB bridge (see src/bridge/db_bridge.rs), so `gil=True` is
no longer needed for basic DB access. Parallelism ceiling is
min(sub_interp_workers, max_connections).

v1 is a sync API — handler threads block on `rt.block_on()` while the
dedicated DB runtime drives the sqlx future. The GIL is released during
the wait, so other workers make progress.

Example::

    from pyronova import Pyronova
    from pyronova.db import PgPool

    app = Pyronova()
    pool = PgPool.connect("postgres://localhost/mydb", max_connections=20)

    @app.get("/users/{id}")
    def get_user(req):
        row = pool.fetch_one(
            "SELECT id, name, email FROM users WHERE id = $1",
            int(req.params["id"]),
        )
        if row is None:
            return Response({"error": "not found"}, 404)
        return row

Supported parameter types: int, float, str, bool, bytes, None, dict
(JSON), list (JSON). Rows decode the same set plus the Postgres type
families int2/int4/int8, float4/float8, text/varchar/char, bytea, bool,
json/jsonb.

For large result sets, use `pool.fetch_iter(sql, ...)` to get a
streaming cursor — O(1) memory, rows yielded one at a time. Streaming
cursors are currently main-interp only (sub-interp proxy is a mock),
so export-style handlers still need `gil=True`:

    @app.get("/export", gil=True)
    def export(req):
        def stream():
            for row in pool.fetch_iter("SELECT * FROM transactions"):
                yield json.dumps(row) + "\\n"
        return Stream(stream())

Deferred to v2: datetime / uuid / decimal types; transactions;
automatic Pydantic model mapping.
"""

from .engine import PgCursor, PgPool

__all__ = ["PgCursor", "PgPool"]
