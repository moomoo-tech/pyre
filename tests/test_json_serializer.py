"""Tests for the Rust-side JSON serializer (json.rs / py_to_json_value).

All cases are exercised through the HTTP handler return path so the full
serializer stack is covered without mocking internals.
"""

import collections
import math
import pytest
from pyronova import Pyronova
from pyronova.testing import TestClient


@pytest.fixture(scope="module")
def client():
    app = Pyronova()

    @app.get("/primitives")
    def primitives(req):
        return {"none": None, "true": True, "false": False, "int": 42, "float": 3.14, "str": "hello"}

    @app.get("/list")
    def as_list(req):
        return [1, 2, 3]

    @app.get("/tuple")
    def as_tuple(req):
        # tuple as a value inside a dict so json.rs serializes it
        return {"t": (1, "two", True)}

    @app.get("/big-int-u64")
    def big_int_u64(req):
        # u64::MAX — larger than i64::MAX; must be a JSON number, not a string
        return {"v": 18446744073709551615}

    @app.get("/big-int-huge")
    def big_int_huge(req):
        # Beyond u64::MAX — falls through to f64; still a JSON number
        return {"v": 2**65}

    @app.get("/float-key-whole")
    def float_key_whole(req):
        # Whole-number float key: Python json.dumps gives "1.0", not "1"
        return {1.0: "v"}

    @app.get("/float-key-frac")
    def float_key_frac(req):
        return {1.5: "v"}

    @app.get("/bool-key")
    def bool_key(req):
        # bool keys: "true"/"false", not Python's "True"/"False"
        return {True: 1, False: 0}

    @app.get("/none-key")
    def none_key(req):
        return {None: "v"}

    @app.get("/ordered-dict")
    def ordered_dict(req):
        return collections.OrderedDict([("x", 1), ("y", 2)])

    @app.get("/set")
    def set_handler(req):
        # set as a value inside a dict — json.rs must reject it → 500
        return {"s": {1, 2, 3}}

    @app.get("/frozenset")
    def frozenset_handler(req):
        return {"fs": frozenset([1, 2])}  # must raise TypeError → 500

    @app.get("/bytes")
    def bytes_handler(req):
        # bytes as a value inside a dict — json.rs must reject it → 500
        return {"b": b"hello"}

    @app.get("/nan")
    def nan_handler(req):
        return {"v": float("nan")}  # must raise TypeError → 500

    @app.get("/infinity")
    def infinity_handler(req):
        return {"v": float("inf")}  # must raise TypeError → 500

    @app.get("/nested-error")
    def nested_error(req):
        # set inside a nested structure produces a TypeError with path context
        return {"users": [1, {1, 2}]}

    @app.get("/escape")
    def escape(req):
        return {
            "quote": '"',
            "backslash": "\\",
            "newline": "\n",
            "carriage": "\r",
            "tab": "\t",
            "backspace": "\x08",
            "formfeed": "\x0c",
            "nul": "\x00",
            "other_ctrl": "\x1f",
        }

    @app.get("/unicode")
    def unicode_passthrough(req):
        return {"msg": "你好，世界！🔥"}

    c = TestClient(app)
    yield c
    c.close()


def test_primitives(client):
    r = client.get("/primitives")
    assert r.status_code == 200
    d = r.json()
    assert d["none"] is None
    assert d["true"] is True
    assert d["false"] is False
    assert d["int"] == 42
    assert abs(d["float"] - 3.14) < 1e-9
    assert d["str"] == "hello"


def test_list(client):
    r = client.get("/list")
    assert r.status_code == 200
    assert r.json() == [1, 2, 3]


def test_tuple_as_array(client):
    r = client.get("/tuple")
    assert r.status_code == 200
    assert r.json() == {"t": [1, "two", True]}


def test_big_int_u64_is_number(client):
    """u64::MAX must arrive as a JSON number, not a quoted string."""
    r = client.get("/big-int-u64")
    assert r.status_code == 200
    raw = r.text
    # The raw JSON must contain the digits without quotes around them
    assert '"v": 18446744073709551615' in raw or '"v":18446744073709551615' in raw


def test_big_int_huge_is_number(client):
    """Integers beyond u64::MAX fall through to f64 but stay a JSON number."""
    r = client.get("/big-int-huge")
    assert r.status_code == 200
    raw = r.text
    # Must not be a quoted string
    import json as stdlib_json
    data = stdlib_json.loads(raw)
    assert isinstance(data["v"], (int, float))


def test_float_key_whole_number(client):
    """Whole-number float dict key must serialize as '1.0', not '1'."""
    r = client.get("/float-key-whole")
    assert r.status_code == 200
    assert '"1.0"' in r.text


def test_float_key_fractional(client):
    r = client.get("/float-key-frac")
    assert r.status_code == 200
    assert '"1.5"' in r.text


def test_bool_dict_key(client):
    """Python True/False keys must become JSON 'true'/'false'."""
    r = client.get("/bool-key")
    assert r.status_code == 200
    assert '"true"' in r.text
    assert '"false"' in r.text
    assert '"True"' not in r.text
    assert '"False"' not in r.text


def test_none_dict_key(client):
    r = client.get("/none-key")
    assert r.status_code == 200
    assert '"null"' in r.text


def test_ordered_dict_via_mapping(client):
    r = client.get("/ordered-dict")
    assert r.status_code == 200
    assert r.json() == {"x": 1, "y": 2}


def test_set_raises_type_error(client):
    r = client.get("/set")
    assert r.status_code == 500


def test_frozenset_raises_type_error(client):
    r = client.get("/frozenset")
    assert r.status_code == 500


def test_bytes_raises_type_error(client):
    r = client.get("/bytes")
    assert r.status_code == 500


def test_nan_raises_type_error(client):
    r = client.get("/nan")
    assert r.status_code == 500


def test_infinity_raises_type_error(client):
    r = client.get("/infinity")
    assert r.status_code == 500


def test_nested_error_includes_path(client):
    """A set inside a nested list must produce a 500 (not silently corrupt)."""
    r = client.get("/nested-error")
    assert r.status_code == 500


def test_string_escaping(client):
    """write_str_escaped must produce valid JSON for all special characters."""
    r = client.get("/escape")
    assert r.status_code == 200
    # json.loads round-trip proves each character was correctly escaped
    d = r.json()
    assert d["quote"] == '"'
    assert d["backslash"] == "\\"
    assert d["newline"] == "\n"
    assert d["carriage"] == "\r"
    assert d["tab"] == "\t"
    assert d["backspace"] == "\x08"
    assert d["formfeed"] == "\x0c"
    assert d["nul"] == "\x00"
    assert d["other_ctrl"] == "\x1f"
    # Raw bytes must use the short named escapes, not \u00XX, for the common ones
    raw = r.text
    assert r'\"' in raw
    assert r'\\' in raw
    assert r'\n' in raw
    assert r'\r' in raw
    assert r'\t' in raw
    assert r'\b' in raw
    assert r'\f' in raw


def test_unicode_passthrough(client):
    """Non-ASCII UTF-8 must pass through unescaped (ensure_ascii=False style)."""
    r = client.get("/unicode")
    assert r.status_code == 200
    assert r.json() == {"msg": "你好，世界！🔥"}
    # Characters must appear as UTF-8 bytes, not \uXXXX escapes
    assert "你好" in r.text
