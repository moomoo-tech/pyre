"""Async Postgres support for Pyre handlers.

Thin Python-side re-export of the Rust `PgPool` class. Initialize once at
startup, then call `pool.fetch_one(...)`, `.fetch_all(...)`, `.fetch_scalar(...)`,
`.execute(...)` from any handler (GIL mode or sub-interpreter).

v1 is a sync API — handler threads block on `rt.block_on()` while the
dedicated DB runtime drives the sqlx future. The GIL is released during
the wait, so other Python threads make progress.

Example::

    from pyreframework import Pyre
    from pyreframework.db import PgPool

    app = Pyre()
    pool = PgPool.connect("postgres://localhost/mydb", max_connections=20)

    @app.get("/users/{id}", gil=True)
    def get_user(req):
        row = pool.fetch_one(
            "SELECT id, name, email FROM users WHERE id = $1",
            int(req.params["id"]),
        )
        if row is None:
            return PyreResponse({"error": "not found"}, 404)
        return row

Supported parameter types: int, float, str, bool, bytes, None, dict
(JSON), list (JSON). Rows decode the same set plus the Postgres type
families int2/int4/int8, float4/float8, text/varchar/char, bytea, bool,
json/jsonb.

Deferred to v2: datetime / uuid / decimal types; `async def` handlers
with `await pool.fetch_one(...)`; transactions; streaming result sets;
automatic Pydantic model mapping.
"""

from .engine import PgPool

__all__ = ["PgPool"]
