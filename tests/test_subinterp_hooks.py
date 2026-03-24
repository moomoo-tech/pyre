"""Test: after_request hooks work in sub-interpreter mode."""
from skytrade import SkyApp

app = SkyApp()


def add_cors(req, resp):
    """after_request hook: add CORS header."""
    # In sub-interp, _SkyResponse is available in globals
    # Return a _SkyResponse with extra headers
    return _SkyResponse(
        body=resp.body,
        status_code=resp.status_code,
        content_type=resp.content_type,
        headers={**resp.headers, "x-cors": "true"},
    )


def fast_route(req):
    return "fast"


def json_route(req):
    return '{"key": "value"}'


app.after_request(add_cors)
app.get("/fast", fast_route)
app.get("/json", json_route)

if __name__ == "__main__":
    app.run(host="127.0.0.1", port=8000, mode="subinterp")
