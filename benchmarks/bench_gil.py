"""Benchmark: GIL mode with decorator syntax + middleware + fallback (no logging)."""

from pyronova import Pyronova, Response

app = Pyronova()


@app.after_request
def add_cors(req, resp):
    return Response(
        body=resp.body,
        status_code=resp.status_code,
        content_type=resp.content_type,
        headers={**resp.headers, "access-control-allow-origin": "*"},
    )


@app.get("/")
def index(req):
    return "Hello from Pyronova!"


@app.get("/hello/{name}")
def greet(req):
    name = req.params.get("name", "world")
    return {"message": f"Hello, {name}!"}


@app.get("/search")
def search(req):
    q = req.query_params.get("q", "")
    ua = req.headers.get("user-agent", "unknown")
    return {"query": q, "user_agent": ua}


@app.fallback
def not_found(req):
    return Response(
        body={"error": "not found", "path": req.path},
        status_code=404,
        content_type="application/json",
    )


if __name__ == "__main__":
    app.run(host="127.0.0.1", port=8000)
