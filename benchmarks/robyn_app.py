"""Robyn comparison server — equivalent to examples/hello.py"""

from robyn import Robyn

app = Robyn(__file__)


@app.get("/")
async def index(request):
    return "Hello from Robyn!"


@app.get("/hello/:name")
async def greet(request):
    name = request.path_params.get("name", "world")
    return f'{{"message": "Hello, {name}!"}}'


if __name__ == "__main__":
    app.start(host="127.0.0.1", port=8001)
