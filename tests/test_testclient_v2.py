"""Tests for TestClient v2 — params, cookies, redirects, OPTIONS/HEAD,
response helpers (.ok, .raise_for_status, .json)."""

from __future__ import annotations

import pytest

from pyronova import Pyronova, Response
from pyronova.testing import TestClient


# ---------------------------------------------------------------------------
# params=
# ---------------------------------------------------------------------------


def test_params_dict_is_urlencoded():
    app = Pyronova()

    @app.get("/q")
    def q(req):
        return {"query": req.query}

    with TestClient(app, port=None) as c:
        r = c.get("/q", params={"limit": 10, "tag": "hello world"})
        assert r.status_code == 200
        q_str = r.json()["query"]
        assert "limit=10" in q_str
        assert "tag=hello+world" in q_str or "tag=hello%20world" in q_str


def test_params_merges_with_existing_query():
    app = Pyronova()

    @app.get("/q")
    def q(req):
        return {"query": req.query}

    with TestClient(app, port=None) as c:
        r = c.get("/q?from=path", params={"add": "param"})
        q_str = r.json()["query"]
        assert "from=path" in q_str
        assert "add=param" in q_str


def test_params_doseq_list():
    app = Pyronova()

    @app.get("/q")
    def q(req):
        return {"query": req.query}

    with TestClient(app, port=None) as c:
        r = c.get("/q", params={"tag": ["a", "b"]})
        q_str = r.json()["query"]
        assert "tag=a" in q_str and "tag=b" in q_str


# ---------------------------------------------------------------------------
# Cookies
# ---------------------------------------------------------------------------


def test_cookies_persist_across_requests():
    app = Pyronova()

    @app.post("/login")
    def login(req):
        return Response(
            body="",
            headers={"Set-Cookie": "sid=abc123; Path=/"},
        )

    @app.get("/whoami")
    def whoami(req):
        return {"cookie": req.headers.get("cookie", "")}

    with TestClient(app, port=None) as c:
        c.post("/login")
        r = c.get("/whoami")
        # Server-set cookie was echoed on the next request.
        assert "sid=abc123" in r.json()["cookie"]


# ---------------------------------------------------------------------------
# Redirects
# ---------------------------------------------------------------------------


def test_follows_redirects_by_default():
    app = Pyronova()

    @app.get("/from")
    def redir(req):
        return Response(body="", status_code=302, headers={"Location": "/to"})

    @app.get("/to")
    def dest(req):
        return "landed"

    with TestClient(app, port=None) as c:
        r = c.get("/from")
        # urllib followed automatically.
        assert r.status_code == 200
        assert r.text == "landed"


def test_can_disable_redirect_following():
    app = Pyronova()

    @app.get("/from")
    def redir(req):
        return Response(body="", status_code=302, headers={"Location": "/to"})

    @app.get("/to")
    def dest(req):
        return "landed"

    with TestClient(app, port=None, follow_redirects=False) as c:
        r = c.get("/from")
        assert r.status_code == 302
        loc = r.headers.get("Location") or r.headers.get("location")
        assert loc == "/to"


# ---------------------------------------------------------------------------
# OPTIONS / HEAD
# ---------------------------------------------------------------------------


def test_options_method():
    app = Pyronova()

    @app.options("/thing")
    def opts(req):
        return Response(body="", status_code=204, headers={"Allow": "GET, POST"})

    with TestClient(app, port=None) as c:
        r = c.options("/thing")
        assert r.status_code == 204
        # urllib lowercases header names in some versions; compare case-insensitively.
        allow = r.headers.get("Allow") or r.headers.get("allow")
        assert allow == "GET, POST"


def test_head_method():
    app = Pyronova()

    @app.head("/ping")
    def head(req):
        return "pong"

    with TestClient(app, port=None) as c:
        r = c.head("/ping")
        assert r.status_code == 200


# ---------------------------------------------------------------------------
# TestResponse helpers
# ---------------------------------------------------------------------------


def test_response_ok_true_on_2xx():
    app = Pyronova()

    @app.get("/")
    def root(req):
        return "ok"

    with TestClient(app, port=None) as c:
        r = c.get("/")
        assert r.ok is True


def test_response_ok_false_on_4xx():
    app = Pyronova()

    @app.get("/")
    def root(req):
        return Response(body="nope", status_code=404)

    with TestClient(app, port=None) as c:
        r = c.get("/")
        assert r.ok is False


def test_raise_for_status_raises_on_error():
    app = Pyronova()

    @app.get("/bad")
    def bad(req):
        return Response(body="boom", status_code=500)

    with TestClient(app, port=None) as c:
        r = c.get("/bad")
        with pytest.raises(RuntimeError):
            r.raise_for_status()


def test_raise_for_status_noop_on_success():
    app = Pyronova()

    @app.get("/")
    def root(req):
        return "ok"

    with TestClient(app, port=None) as c:
        r = c.get("/")
        r.raise_for_status()  # must not raise


def test_json_accepts_list_response():
    app = Pyronova()

    @app.get("/nums")
    def nums(req):
        return [1, 2, 3]

    with TestClient(app, port=None) as c:
        r = c.get("/nums")
        assert r.json() == [1, 2, 3]


# ---------------------------------------------------------------------------
# Low-level request()
# ---------------------------------------------------------------------------


def test_request_method_accepts_arbitrary_verbs():
    app = Pyronova()

    @app.route("PATCH", "/thing")
    def patch(req):
        return "patched"

    with TestClient(app, port=None) as c:
        r = c.request("PATCH", "/thing")
        assert r.text == "patched"
