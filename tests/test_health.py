"""Tests for /livez and /readyz health probes."""

from __future__ import annotations

import json

import pytest

from pyronova import Pyronova
from pyronova.testing import TestClient


def test_livez_returns_200_always():
    app = Pyronova()
    app.enable_health_probes()
    with TestClient(app, port=None) as c:
        r = c.get("/livez")
        assert r.status_code == 200
        assert r.json() == {"status": "alive"}


def test_readyz_ok_when_no_checks():
    app = Pyronova()
    app.enable_health_probes()
    with TestClient(app, port=None) as c:
        r = c.get("/readyz")
        assert r.status_code == 200
        data = r.json()
        assert data == {"status": "ready", "checks": {}}


def test_readyz_ok_with_passing_checks():
    app = Pyronova()

    @app.readiness_check("always_ok")
    def _():
        return True

    @app.readiness_check("none_also_ok")
    def _():
        return None  # None is fine — only False / exception fail

    app.enable_health_probes()
    with TestClient(app, port=None) as c:
        r = c.get("/readyz")
        assert r.status_code == 200
        data = r.json()
        assert data["status"] == "ready"
        assert data["checks"]["always_ok"] == {"ok": True}
        assert data["checks"]["none_also_ok"] == {"ok": True}


def test_readyz_503_on_exception():
    app = Pyronova()

    @app.readiness_check("db")
    def _():
        raise RuntimeError("connection refused")

    app.enable_health_probes()
    with TestClient(app, port=None) as c:
        r = c.get("/readyz")
        assert r.status_code == 503
        data = r.json()
        assert data["status"] == "not_ready"
        assert data["checks"]["db"]["ok"] is False
        assert "connection refused" in data["checks"]["db"]["error"]
        assert "RuntimeError" in data["checks"]["db"]["error"]


def test_readyz_503_on_false_return():
    app = Pyronova()

    @app.readiness_check("feature_flag")
    def _():
        return False

    app.enable_health_probes()
    with TestClient(app, port=None) as c:
        r = c.get("/readyz")
        assert r.status_code == 503
        assert r.json()["checks"]["feature_flag"]["ok"] is False


def test_readyz_aggregates_multiple_checks():
    app = Pyronova()

    @app.readiness_check("ok1")
    def _():
        return True

    @app.readiness_check("bad")
    def _():
        raise ValueError("nope")

    @app.readiness_check("ok2")
    def _():
        return "healthy"

    app.enable_health_probes()
    with TestClient(app, port=None) as c:
        r = c.get("/readyz")
        # One failure → overall 503, but every check is reported.
        assert r.status_code == 503
        checks = r.json()["checks"]
        assert checks["ok1"]["ok"] is True
        assert checks["ok2"]["ok"] is True
        assert checks["bad"]["ok"] is False


def test_async_readiness_check_supported():
    app = Pyronova()

    @app.readiness_check("async_ok")
    async def _():
        return True

    @app.readiness_check("async_fail")
    async def _():
        raise ConnectionError("timeout")

    app.enable_health_probes()
    with TestClient(app, port=None) as c:
        r = c.get("/readyz")
        assert r.status_code == 503
        data = r.json()
        assert data["checks"]["async_ok"]["ok"] is True
        assert data["checks"]["async_fail"]["ok"] is False
        assert "timeout" in data["checks"]["async_fail"]["error"]


def test_enable_health_probes_idempotent():
    app = Pyronova()
    app.enable_health_probes()
    # Second call is a no-op — no duplicate route registration error.
    app.enable_health_probes()
    with TestClient(app, port=None) as c:
        assert c.get("/livez").status_code == 200


def test_custom_paths():
    app = Pyronova()
    app.enable_health_probes(livez_path="/_alive", readyz_path="/_ready")
    with TestClient(app, port=None) as c:
        assert c.get("/_alive").status_code == 200
        assert c.get("/_ready").status_code == 200
        # Default paths are NOT registered.
        assert c.get("/livez").status_code == 404


def test_check_registered_after_enable_still_runs():
    """You can enable probes early (e.g., in Pyronova() setup) and register
    checks later as modules load. The readyz handler closes over the
    shared list, so late appends take effect immediately."""
    app = Pyronova()
    app.enable_health_probes()

    @app.readiness_check("late")
    def _():
        raise RuntimeError("late check ran")

    with TestClient(app, port=None) as c:
        r = c.get("/readyz")
        assert r.status_code == 503
        assert "late check ran" in r.json()["checks"]["late"]["error"]
