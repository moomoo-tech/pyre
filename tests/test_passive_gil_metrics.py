"""Tests for passive GIL contention monitoring.

The GIL watchdog was replaced with passive measurement: each request
handler records GIL acquisition wait time as a byproduct, eliminating
the active probe thread (zero observer-effect overhead).

Covers:
- get_gil_metrics() returns correct 9-element tuple
- Probe count increments with real requests
- GIL hold peak tracks CPU-heavy handlers
- Total requests counter works
"""

import os

import pytest
from pyronova import Pyronova, get_gil_metrics
from pyronova.testing import TestClient


@pytest.fixture(scope="module")
def client():
    # TOTAL_REQUESTS counter is gated by PYRONOVA_METRICS=1 (default off
    # to keep the cross-core atomic out of the 5M req/s hot path). Tests
    # that read metrics[8] need it on; flip before TestClient spawns the
    # server thread so the Rust-side init_metrics_flag() picks it up.
    os.environ["PYRONOVA_METRICS"] = "1"
    app = Pyronova()

    @app.get("/")
    def index(req):
        return {"ok": True}

    @app.get("/heavy")
    def heavy(req):
        """Simulate a handler that holds the GIL for a measurable time."""
        total = 0
        for i in range(200_000):
            total += i
        return {"total": total}

    c = TestClient(app, port=19892)
    yield c
    c.close()


def test_metrics_tuple_shape():
    """get_gil_metrics() returns a 9-element tuple of integers."""
    m = get_gil_metrics()
    assert isinstance(m, tuple)
    assert len(m) == 9


def test_probe_count_increments(client):
    """After requests, passive probe count should reflect handler invocations."""
    # Reset peaks by reading
    get_gil_metrics()

    for _ in range(10):
        resp = client.get("/")
        assert resp.status_code == 200

    metrics = get_gil_metrics()
    # metrics[2] = probe_count
    assert metrics[2] >= 10, f"Expected >= 10 probes, got {metrics[2]}"


def test_total_requests_counter(client):
    """TOTAL_REQUESTS counter increments with each request."""
    before = get_gil_metrics()[8]  # total_requests

    for _ in range(5):
        client.get("/")

    after = get_gil_metrics()[8]
    assert after >= before + 5, f"Expected at least +5, got {after - before}"


def test_heavy_handler_hold_peak(client):
    """CPU-heavy handler should produce higher GIL hold times."""
    # Reset peaks
    get_gil_metrics()

    resp = client.get("/heavy")
    assert resp.status_code == 200

    metrics = get_gil_metrics()
    hold_peak_us = metrics[6]  # hold_peak_us
    # CPU loop should hold GIL for at least a few hundred microseconds
    assert hold_peak_us > 100, f"Expected hold_peak > 100us, got {hold_peak_us}"
