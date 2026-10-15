import pytest

from knowledge import data
from knowledge.data import ParseError


def test_toml():
    text = '# c\n[[path]]\nprefix = "~/p"\nscopes = ["org/x"]\n[budget]\n"lang/python" = [10, 4000]\nd = 2026-10-06\n'
    doc = data.load_toml(text)
    assert doc == {
        "path": [{"prefix": "~/p", "scopes": ["org/x"]}],
        "budget": {"lang/python": [10, 4000], "d": "2026-10-06"},
    }


def test_bad_toml_is_a_parse_error():
    with pytest.raises(ParseError):
        data.load_toml("a = ")


def test_json_loads_cleans_surrogates():
    assert data.loads('"a\\ud800b"') == "a?b"
    with pytest.raises(ParseError):
        data.loads("{")


def test_dumps_is_compact_by_default():
    assert data.dumps({"a": [1, "é"]}) == '{"a":[1,"é"]}'
    assert data.dumps({"a": 1}, indent=2) == '{\n  "a": 1\n}'


def test_checked_access():
    o = data.as_obj(data.loads('{"s": "x", "n": 4, "l": ["a", "b"], "b": true, "o": {"k": 1}}'))
    assert data.get_str(o, "s") == "x"
    assert data.str_list(o, "l") == ["a", "b"] and data.get_bool(o, "b")
    assert data.opt_str(o, "missing") is None and data.str_or(o, "missing", "d") == "d"
    assert data.opt_list(o, "missing") == []
    for bad in (lambda: data.get_str(o, "n"), lambda: data.get_str(o, "missing"),
                lambda: data.str_list(o, "o"), lambda: data.get_bool(o, "n"), lambda: data.opt_list(o, "s")):
        with pytest.raises(ParseError):
            bad()
