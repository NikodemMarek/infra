import pytest

from secret_broker import data
from secret_broker.data import ParseError


def test_loads_ok() -> None:
    assert data.loads('{"a":[1,"x",null]}') == {"a": [1, "x", None]}
    assert data.loads(b'{"a":1}') == {"a": 1}


@pytest.mark.parametrize("text", ["NaN", "Infinity", "-Infinity", "{", "", "[1,]"])
def test_loads_rejects(text: str) -> None:
    with pytest.raises(ParseError):
        data.loads(text)


def test_loads_rejects_lone_surrogate() -> None:
    with pytest.raises(ParseError):
        data.loads('"\\ud800"')


def test_loads_rejects_invalid_utf8() -> None:
    with pytest.raises(ParseError):
        data.loads(b'"\xff"')


def test_dumps_compact_unicode() -> None:
    assert data.dumps({"a": [1, "é"], "b": None}) == '{"a":[1,"é"],"b":null}'


def test_toml_loads_ok() -> None:
    assert data.toml_loads('a = 1\n[t]\nb = "x"') == {"a": 1, "t": {"b": "x"}}


def test_toml_invalid() -> None:
    with pytest.raises(ParseError, match="invalid TOML"):
        data.toml_loads("[x")


@pytest.mark.parametrize(
    "text",
    [
        "a = 1979-05-27",
        "a = 1979-05-27T07:32:00Z",
        "a = 07:32:00",
        "a = 1979-05-27T07:32:00",
        "[t]\nb = [1979-05-27]",
        "[t.u]\nb = { c = 1979-05-27 }",
    ],
)
def test_toml_dates_rejected(text: str) -> None:
    with pytest.raises(ParseError, match="unsupported TOML value"):
        data.toml_loads(text)


def test_as_obj_as_list() -> None:
    assert data.as_obj({"a": 1}) == {"a": 1}
    assert data.as_list([1]) == [1]
    with pytest.raises(ParseError):
        data.as_obj([1], "thing")
    with pytest.raises(ParseError):
        data.as_obj(None)
    with pytest.raises(ParseError):
        data.as_list({"a": 1})
    with pytest.raises(ParseError):
        data.as_list("x")


def test_get_str() -> None:
    assert data.get_str({"k": "v"}, "k") == "v"
    with pytest.raises(ParseError, match="k: expected .+, got NoneType"):
        data.get_str({}, "k")
    with pytest.raises(ParseError, match="k: expected .+, got int"):
        data.get_str({"k": 1}, "k")


def test_opt_str() -> None:
    assert data.opt_str({"k": "v"}, "k", None) == "v"
    assert data.opt_str({}, "k", None) is None
    assert data.opt_str({}, "k", "d") == "d"
    assert data.opt_str({"k": None}, "k", "d") == "d"
    with pytest.raises(ParseError):
        data.opt_str({"k": 1}, "k", None)


def test_opt_int() -> None:
    assert data.opt_int({"k": 5}, "k", 1) == 5
    assert data.opt_int({}, "k", 1) == 1
    for bad in (True, False, "1", 1.5, [1]):
        with pytest.raises(ParseError):
            data.opt_int({"t": bad}, "t", 1)


def test_opt_bool() -> None:
    assert data.opt_bool({"k": False}, "k", True) is False
    assert data.opt_bool({}, "k", True) is True
    for bad in (1, 0, "true"):
        with pytest.raises(ParseError):
            data.opt_bool({"k": bad}, "k", True)


def test_str_list() -> None:
    assert data.str_list(["a", "b"], "x") == ["a", "b"]
    assert data.str_list([], "x") == []
    for bad in ("a", None, ["a", 1], {"a": "b"}):
        with pytest.raises(ParseError):
            data.str_list(bad, "x")


def test_str_dict() -> None:
    assert data.str_dict({"a": "b"}, "x") == {"a": "b"}
    assert data.str_dict({}, "x") == {}
    for bad in ("a", None, {"a": 1}, ["a"]):
        with pytest.raises(ParseError):
            data.str_dict(bad, "x")


def test_b64() -> None:
    assert data.b64(b"\x00\xff") == "AP8="
    assert data.b64(b"") == ""
    assert data.unb64(data.b64(b"\x00\xff")) == b"\x00\xff"
    assert data.unb64("") == b""


@pytest.mark.parametrize("text", ["%%", "AP8", "A"])
def test_unb64_invalid(text: str) -> None:
    with pytest.raises(ParseError):
        data.unb64(text)


@pytest.mark.parametrize(
    "payload",
    [
        "1" * 5000,
        "[" * 100000,
        b'"\xff\xfe"',
        b"\x80",
        "1e999",
        "-1e999",
        '{"a":' + "1" * 5000 + "}",
        "[" * 100000 + "]" * 100000,
    ],
)
def test_loads_hostile_payloads(payload: str | bytes) -> None:
    with pytest.raises(ParseError):
        data.loads(payload)


@pytest.mark.parametrize("text", ["a = " + "1" * 5000, "a = " + "[" * 100000, "a = " + "[" * 100000 + "]" * 100000])
def test_toml_loads_hostile_payloads(text: str) -> None:
    with pytest.raises(ParseError):
        data.toml_loads(text)
