"""SkyTrade / Pyre — A high-performance Python web framework powered by Rust."""

from skytrade.engine import SkyApp, SkyRequest, SkyResponse, SkyWebSocket, SharedState, SkyStream, get_gil_metrics
from skytrade.app import Pyre
from skytrade.rpc import PyreRPCClient

__all__ = ["Pyre", "SkyApp", "SkyRequest", "SkyResponse", "SkyWebSocket", "SharedState", "SkyStream", "get_gil_metrics"]
try:
    from importlib.metadata import version as _get_version
    __version__ = _get_version("skytrade")
except Exception:
    __version__ = "dev"
