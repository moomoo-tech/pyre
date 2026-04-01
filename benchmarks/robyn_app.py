"""Robyn comparison server — matches Pyre bench routes."""

from robyn import Robyn
import json
import os

app = Robyn(__file__)

PORT = int(os.environ.get("ROBYN_PORT", "8001"))


@app.get("/")
async def index(request):
    return "Hello from Robyn!"


@app.get("/json")
async def json_route(request):
    return json.dumps({"message": "hello", "status": "ok", "code": 200})


@app.get("/user/:id")
async def user(request):
    uid = request.path_params.get("id", "0")
    ip = request.ip_addr or ""
    return json.dumps({"id": uid, "ip": ip})


@app.get("/user/:id/post/:post_id")
async def user_post(request):
    uid = request.path_params.get("id", "0")
    pid = request.path_params.get("post_id", "0")
    return json.dumps({"user": uid, "post": pid})


@app.post("/echo")
async def echo(request):
    return request.body


@app.get("/headers")
async def headers(request):
    host = request.headers.get("host", "")
    ua = request.headers.get("user-agent", "")
    return json.dumps({"host": host, "ua": ua})


@app.get("/compute")
async def compute(request):
    total = 0
    for i in range(100):
        total += i * i
    return json.dumps({"result": total})


if __name__ == "__main__":
    app.start(host="127.0.0.1", port=PORT)
