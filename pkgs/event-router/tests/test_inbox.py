from pathlib import Path

import pytest

from event_router.data import Obj
from event_router.event import Event
from event_router.inbox import CAP, Inbox, Item, expired, matches


def ev(type_: str, **payload: object) -> Event:
    return Event.from_json({"type": type_, **payload})  # type: ignore[arg-type]


@pytest.mark.parametrize(
    ("type_", "expected"),
    [
        ("claude", True),
        ("claude.question", True),
        ("claude.a.b", True),
        ("claudex.q", False),
        ("claudex", False),
        ("other.claude", False),
    ],
)
def test_matches(type_: str, expected: bool) -> None:
    assert matches(["claude"], type_) is expected


def test_matches_any_prefix() -> None:
    assert matches(["a", "claude"], "claude.q")
    assert not matches([], "claude")


def test_non_matching_not_queued() -> None:
    inbox = Inbox(["claude"])
    assert inbox.add(ev("other.x"), 1.0) is None
    assert inbox.items(1.0) == []


def test_insertion_order_and_json() -> None:
    inbox = Inbox(["claude"])
    a = inbox.add(ev("claude.a"), 1.0)
    b = inbox.add(ev("claude.b"), 2.0)
    assert a is not None and b is not None and a != b
    items = inbox.items(3.0)
    assert [i.id for i in items] == [a, b]
    assert items[0].to_json() == {"id": a, "received": 1.0, "event": {"type": "claude.a"}}


def test_cap_drops_oldest() -> None:
    inbox = Inbox(["claude"])
    ids = [inbox.add(ev("claude.x", n=n), float(n)) for n in range(CAP + 3)]
    items = inbox.items(float(CAP + 3))
    assert len(items) == CAP
    assert [i.id for i in items] == ids[3:]


def test_timeout_expiry() -> None:
    inbox = Inbox(["claude"])
    inbox.add(ev("claude.x", timeout=10), 100.0)
    assert len(inbox.items(110.0)) == 1
    assert inbox.items(110.5) == []


@pytest.mark.parametrize("timeout", [True, "5", None, [1], {"a": 1}])
def test_timeout_ignored_unless_number(timeout: object) -> None:
    item = Item("i", 0.0, ev("claude.x", timeout=timeout))
    assert not expired(item, 1e9)


def test_reply_expiry(tmp_path: Path) -> None:
    reply = tmp_path / "reply"
    reply.touch()
    inbox = Inbox(["claude"])
    inbox.add(ev("claude.x", reply=str(reply)), 1.0)
    assert len(inbox.items(2.0)) == 1
    reply.unlink()
    assert inbox.items(3.0) == []


def test_reply_empty_or_non_string_ignored() -> None:
    for reply in ["", 5, None]:
        assert not expired(Item("i", 0.0, ev("claude.x", reply=reply)), 1.0)


def test_add_prunes_expired() -> None:
    inbox = Inbox(["claude"])
    inbox.add(ev("claude.old", timeout=1), 0.0)
    new = inbox.add(ev("claude.new"), 10.0)
    assert [i.id for i in inbox.items(10.0)] == [new]


def test_remove() -> None:
    inbox = Inbox(["claude"])
    i = inbox.add(ev("claude.x"), 1.0)
    assert i is not None
    assert inbox.remove(i) is True
    assert inbox.remove(i) is False
    assert inbox.items(2.0) == []


def test_to_json_event_is_the_payload() -> None:
    data: Obj = {"type": "claude.x", "title": "t"}
    assert Item("i", 1.0, Event.from_json(data)).to_json()["event"] == data
