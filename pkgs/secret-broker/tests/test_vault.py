import pytest

from secret_broker.data import Obj
from secret_broker.vault import ItemError, pick


def reason(results: list[Obj], name: str) -> str:
    with pytest.raises(ItemError) as e:
        pick(results, name)
    return str(e.value)


def test_exact_match_among_similar() -> None:
    results: list[Obj] = [
        {"name": "a-old", "type": 1, "login": {"password": "x"}},
        {"name": "a", "type": 1, "login": {"password": "p4ssw0rd"}},
    ]
    assert pick(results, "a") == "p4ssw0rd"


def test_missing() -> None:
    assert reason([], "a") == "item missing: a"


def test_case_sensitive() -> None:
    results: list[Obj] = [{"name": "a", "type": 1, "login": {"password": "x"}}]
    assert reason(results, "A") == "item missing: A"


def test_ambiguous() -> None:
    results: list[Obj] = [
        {"name": "a", "type": 1, "login": {"password": "x"}},
        {"name": "a", "type": 2, "notes": "y"},
    ]
    assert reason(results, "a") == "item ambiguous: a"


def test_secure_note() -> None:
    assert pick([{"name": "n", "type": 2, "notes": "n0tes-value"}], "n") == "n0tes-value"


@pytest.mark.parametrize(
    "item",
    [
        {"name": "a", "type": 1, "login": {"password": None}},
        {"name": "a", "type": 1, "login": {"password": ""}},
        {"name": "a", "type": 1, "login": {}},
        {"name": "a", "type": 1, "login": None},
        {"name": "a", "type": 1},
        {"name": "a", "type": 1, "login": {"password": ""}, "notes": "fallback"},
        {"name": "a", "type": 2, "notes": ""},
        {"name": "a", "type": 2, "notes": None},
        {"name": "a", "type": 2},
        {"name": "a", "type": 3, "notes": "some notes"},
        {"name": "a", "type": 4, "notes": "some notes"},
        {"name": "a", "type": 5, "notes": "some notes"},
    ],
)
def test_no_value(item: Obj) -> None:
    assert reason([item], "a") == "item has no value: a"
