"""Routing behaviour through the full HTTP stack.

Split out from the old test_all_features.py: basic GET, path params,
query params, nested params, custom status codes, redirect.
"""

import http.client
import json
import os

from tests.conftest import feature_server_factory

SERVER = '''
import os
from pyronova import Pyronova, Response, redirect

app = Pyronova()

@app.get("/__ping")
def ping(req):
    return "pong"

@app.get("/")
def index(req):
    return {"mode": os.environ["PYRONOVA_MODE"]}

@app.get("/hello/{name}")
def hello(req):
    return {"name": req.params.get("name")}

@app.get("/query")
def q(req):
    return {"q": req.query_params.get("q", "")}

@app.get("/nested/{a}/to/{b}")
def nested(req):
    return {"a": req.params["a"], "b": req.params["b"]}

@app.get("/error")
def err(req):
    return Response(body="nope", status_code=404)

@app.get("/redirect")
def rd(req):
    return redirect("/")

if __name__ == "__main__":
    app.run(
        host="127.0.0.1",
        port=int(os.environ["PYRONOVA_PORT"]),
        mode=os.environ["PYRONOVA_MODE"],
    )
'''

feature_server = feature_server_factory(SERVER)


def test_basic_get(feature_server):
    status, body, _ = feature_server.get("/")
    assert status == 200
    assert json.loads(body)["mode"] == feature_server.mode


def test_path_params(feature_server):
    status, body, _ = feature_server.get("/hello/pyronova")
    assert status == 200
    assert json.loads(body)["name"] == "pyronova"


def test_query_params(feature_server):
    status, body, _ = feature_server.get("/query?q=test")
    assert status == 200
    assert json.loads(body)["q"] == "test"


def test_nested_path_params(feature_server):
    status, body, _ = feature_server.get("/nested/foo/to/bar")
    assert status == 200
    data = json.loads(body)
    assert data["a"] == "foo"
    assert data["b"] == "bar"


def test_custom_404_response(feature_server):
    status, body, _ = feature_server.get("/error")
    assert status == 404
    assert body == "nope"


def test_redirect_302(feature_server):
    # Use http.client so we can see the 302 response without following.
    url = feature_server.base_url
    host, port = url.replace("http://", "").split(":")
    conn = http.client.HTTPConnection(host, int(port), timeout=5)
    try:
        conn.request("GET", "/redirect")
        resp = conn.getresponse()
        assert resp.status == 302
        assert resp.getheader("location") == "/"
    finally:
        conn.close()
