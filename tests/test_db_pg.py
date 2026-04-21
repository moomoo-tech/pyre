"""End-to-end Postgres tests.

Skipped entirely unless `PYRE_TEST_PG_DSN` is set. To run locally::

    docker run --rm -d --name pyre-pg -p 5433:5432 \\
        -e POSTGRES_PASSWORD=pyre -e POSTGRES_DB=pyretest postgres:17-alpine

    export PYRE_TEST_PG_DSN="postgres://postgres:pyre@127.0.0.1:5433/pyretest"
    pytest tests/test_db_pg.py

The pool is a global in the Rust side, which means these tests must share
a single `PgPool.connect()` per process. pytest's default process-per-run
model is fine — all tests here use a module-scoped fixture that sets up
a schema once.
"""

import os
import json
import threading
import time
import urllib.request

import pytest

from pyreframework import Pyre
from pyreframework.db import PgPool
from pyreframework.testing import TestClient


PG_DSN = os.environ.get("PYRE_TEST_PG_DSN")

pytestmark = pytest.mark.skipif(
    PG_DSN is None,
    reason="PYRE_TEST_PG_DSN not set — skipping Postgres integration tests",
)


@pytest.fixture(scope="module")
def pool():
    # Rust-side PgPool is a process global; connect() is idempotent.
    p = PgPool.connect(PG_DSN, max_connections=4)
    # Fresh schema per test module.
    p.execute("DROP TABLE IF EXISTS pyre_test_rows")
    p.execute("""
        CREATE TABLE pyre_test_rows (
            id SERIAL PRIMARY KEY,
            name TEXT NOT NULL,
            value INTEGER,
            flag BOOLEAN DEFAULT false,
            meta JSONB,
            bin BYTEA
        )
    """)
    yield p
    p.execute("DROP TABLE IF EXISTS pyre_test_rows")


# ---------------------------------------------------------------------------
# Raw pool API
# ---------------------------------------------------------------------------

def test_execute_returns_rows_affected(pool):
    n = pool.execute(
        "INSERT INTO pyre_test_rows (name, value) VALUES ($1, $2)",
        "alice", 42,
    )
    assert n == 1


def test_fetch_one_returns_dict(pool):
    pool.execute("INSERT INTO pyre_test_rows (name, value) VALUES ($1, $2)", "bob", 7)
    row = pool.fetch_one("SELECT name, value FROM pyre_test_rows WHERE name = $1", "bob")
    assert row == {"name": "bob", "value": 7}


def test_fetch_one_none_on_no_match(pool):
    row = pool.fetch_one("SELECT name FROM pyre_test_rows WHERE name = $1", "nobody")
    assert row is None


def test_fetch_all_returns_list(pool):
    pool.execute("DELETE FROM pyre_test_rows")
    for i in range(3):
        pool.execute("INSERT INTO pyre_test_rows (name, value) VALUES ($1, $2)", f"n{i}", i)
    rows = pool.fetch_all("SELECT name, value FROM pyre_test_rows ORDER BY value")
    assert [(r["name"], r["value"]) for r in rows] == [("n0", 0), ("n1", 1), ("n2", 2)]


def test_fetch_scalar(pool):
    count = pool.fetch_scalar("SELECT COUNT(*) FROM pyre_test_rows")
    assert isinstance(count, int)
    assert count >= 0


def test_null_values(pool):
    pool.execute("INSERT INTO pyre_test_rows (name, value) VALUES ($1, $2)", "nullish", None)
    row = pool.fetch_one("SELECT name, value FROM pyre_test_rows WHERE name = $1", "nullish")
    assert row == {"name": "nullish", "value": None}


def test_bool_and_json(pool):
    pool.execute(
        "INSERT INTO pyre_test_rows (name, flag, meta) VALUES ($1, $2, $3)",
        "mix", True, {"a": 1, "b": [1, 2, 3]},
    )
    row = pool.fetch_one(
        "SELECT flag, meta FROM pyre_test_rows WHERE name = $1", "mix"
    )
    assert row["flag"] is True
    assert row["meta"] == {"a": 1, "b": [1, 2, 3]}


def test_bytes_roundtrip(pool):
    blob = bytes(range(256))
    pool.execute(
        "INSERT INTO pyre_test_rows (name, bin) VALUES ($1, $2)", "blob", blob
    )
    row = pool.fetch_one("SELECT bin FROM pyre_test_rows WHERE name = $1", "blob")
    assert row["bin"] == blob


# ---------------------------------------------------------------------------
# Handler integration
# ---------------------------------------------------------------------------

def test_handler_can_query(pool):
    # Pre-seed data
    pool.execute("DELETE FROM pyre_test_rows")
    pool.execute("INSERT INTO pyre_test_rows (name, value) VALUES ($1, $2)", "carol", 100)

    app = Pyre()

    @app.get("/")
    def root(req):
        return "ok"

    @app.get("/users/{name}", gil=True)
    def get_user(req):
        return pool.fetch_one(
            "SELECT name, value FROM pyre_test_rows WHERE name = $1",
            req.params["name"],
        ) or {"error": "not found"}

    with TestClient(app, port=None) as c:
        resp = c.get("/users/carol")
        assert resp.status_code == 200
        assert resp.json() == {"name": "carol", "value": 100}

        resp = c.get("/users/nobody")
        assert resp.json() == {"error": "not found"}


def test_unsupported_param_type_raises(pool):
    with pytest.raises(ValueError, match="unsupported parameter type"):
        pool.fetch_one("SELECT $1::text", object())


def test_connect_is_idempotent(pool):
    """Calling connect() again with same DSN returns a usable handle."""
    again = PgPool.connect(PG_DSN, max_connections=1)
    # Uses the same underlying pool.
    assert again.fetch_scalar("SELECT 1") == 1
