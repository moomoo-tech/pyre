"""Tests for the `pyre` CLI.

We test the three pieces we control:

- ``routes`` subcommand prints the route table.
- ``_load_app`` resolves ``module[:attr]``, errors helpfully when the
  target is missing, not a Pyre, or the module fails to import.
- ``--version`` exits cleanly.

We don't exercise ``run``/``dev`` here — those block on the server loop
and are covered by the existing e2e/server tests.
"""

from __future__ import annotations

import os
import sys
import subprocess
import textwrap
from pathlib import Path

import pytest

from pyreframework import Pyre
from pyreframework.cli import _load_app, main


# ---------------------------------------------------------------------------
# _load_app
# ---------------------------------------------------------------------------


def _write_app_module(tmp_path: Path, body: str = "") -> str:
    mod = tmp_path / "cli_fixture_app.py"
    mod.write_text(
        "from pyreframework import Pyre\n"
        "app = Pyre()\n"
        "\n"
        "@app.get('/ping')\n"
        "def ping(req):\n"
        "    return 'pong'\n"
        "\n"
        "@app.post('/upload', gil=True, stream=True)\n"
        "def upload(req):\n"
        "    return ''\n"
        "\n"
        "app.add_fast_response('GET', '/health', b'{\"ok\":true}', content_type='application/json')\n"
        + body
    )
    sys.path.insert(0, str(tmp_path))
    return "cli_fixture_app"


@pytest.fixture(autouse=True)
def _cleanup_sys_path():
    original = list(sys.path)
    mods = set(sys.modules)
    yield
    sys.path[:] = original
    for m in set(sys.modules) - mods:
        del sys.modules[m]


def test_load_app_default_attr(tmp_path):
    modname = _write_app_module(tmp_path)
    app = _load_app(modname)
    assert isinstance(app, Pyre)


def test_load_app_explicit_attr(tmp_path):
    modname = _write_app_module(tmp_path)
    app = _load_app(f"{modname}:app")
    assert isinstance(app, Pyre)


def test_load_app_missing_module_exits(tmp_path):
    with pytest.raises(SystemExit) as ei:
        _load_app("definitely_not_a_module_abc123")
    assert "cannot import" in str(ei.value)


def test_load_app_missing_attr(tmp_path):
    modname = _write_app_module(tmp_path)
    with pytest.raises(SystemExit) as ei:
        _load_app(f"{modname}:not_there")
    assert "no attribute" in str(ei.value)


def test_load_app_wrong_type(tmp_path):
    mod = tmp_path / "cli_not_pyre.py"
    mod.write_text("app = 42\n")
    sys.path.insert(0, str(tmp_path))
    with pytest.raises(SystemExit) as ei:
        _load_app("cli_not_pyre:app")
    assert "expected pyreframework.Pyre" in str(ei.value)


# ---------------------------------------------------------------------------
# routes subcommand
# ---------------------------------------------------------------------------


def test_routes_prints_registered(tmp_path, capsys):
    modname = _write_app_module(tmp_path)
    main(["routes", modname])
    out = capsys.readouterr().out
    assert "GET" in out
    assert "/ping" in out
    assert "/upload" in out
    assert "stream" in out  # flag rendered
    assert "gil" in out
    assert "/health" in out  # fast route
    assert "<fast:" in out  # fast marker


def test_routes_empty_app(tmp_path, capsys):
    mod = tmp_path / "cli_empty.py"
    mod.write_text("from pyreframework import Pyre\napp = Pyre()\n")
    sys.path.insert(0, str(tmp_path))
    main(["routes", "cli_empty"])
    assert "(no routes registered)" in capsys.readouterr().out


# ---------------------------------------------------------------------------
# Top-level argparse
# ---------------------------------------------------------------------------


def test_version_flag(capsys):
    with pytest.raises(SystemExit) as ei:
        main(["--version"])
    assert ei.value.code == 0
    out = capsys.readouterr().out
    assert out.startswith("pyre ")


def test_no_subcommand_errors(capsys):
    with pytest.raises(SystemExit):
        main([])


# ---------------------------------------------------------------------------
# Module entry point
# ---------------------------------------------------------------------------


def test_module_entry_point_runs(tmp_path):
    """`python -m pyreframework routes ...` should work without install."""
    modname = _write_app_module(tmp_path)
    env = os.environ.copy()
    env["PYTHONPATH"] = str(tmp_path) + os.pathsep + env.get("PYTHONPATH", "")
    result = subprocess.run(
        [sys.executable, "-m", "pyreframework", "routes", modname],
        capture_output=True, text=True, env=env, timeout=30,
    )
    assert result.returncode == 0, result.stderr
    assert "/ping" in result.stdout
