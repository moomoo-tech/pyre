"""Minimal Pyre example – start and hit http://127.0.0.1:8000/"""

from skytrade import SkyApp

app = SkyApp()


def index(req):
    return "Hello from Pyre!"


def greet(req):
    name = req.params.get("name", "world")
    return {"message": f"Hello, {name}!"}  # dict → Rust serde_json serialization


def echo(req):
    return req.text()


app.get("/", index)
app.get("/hello/{name}", greet)
app.post("/echo", echo)

if __name__ == "__main__":
    app.run(host="127.0.0.1", port=8000)
