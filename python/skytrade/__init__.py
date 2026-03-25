"""SkyTrade / Pyre — A high-performance Python web framework powered by Rust."""

from skytrade.engine import SkyApp, SkyRequest, SkyResponse, SkyWebSocket, SharedState, SkyStream, get_gil_metrics
from skytrade.app import Pyre
from skytrade.rpc import PyreRPCClient
from skytrade.cookies import get_cookies, get_cookie, set_cookie, delete_cookie
from skytrade.uploads import parse_multipart, UploadFile


def redirect(url: str, status_code: int = 302) -> SkyResponse:
    """Return a redirect response.

    Usage::

        @app.get("/old")
        def old_page(req):
            return redirect("/new")

        @app.get("/permanent")
        def moved(req):
            return redirect("/new-home", status_code=301)
    """
    return SkyResponse(
        body="",
        status_code=status_code,
        headers={"location": url},
    )

__all__ = ["Pyre", "SkyApp", "SkyRequest", "SkyResponse", "SkyWebSocket", "SharedState", "SkyStream", "get_gil_metrics"]
try:
    from importlib.metadata import version as _get_version
    __version__ = _get_version("skytrade")
except Exception:
    __version__ = "dev"
