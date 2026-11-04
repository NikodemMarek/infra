import re

from secret_broker.model import Action


def action(secrets: dict[str, str]) -> Action:
    return Action("a", "d", ("/nix/store/x",), (re.compile("x"),), secrets, 10)


def test_items_sorted_unique() -> None:
    a = action({"A": "zeta", "B": "alpha", "C": "zeta"})
    assert a.items() == ("alpha", "zeta")


def test_items_empty() -> None:
    assert action({}).items() == ()
