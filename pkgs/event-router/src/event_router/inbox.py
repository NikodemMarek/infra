"""In-memory queue of events for a client to act on: kept until removed or expired.

An event is queued when its type matches a configured prefix, and expires on
its own when it says so: `timeout` (seconds since it was received) or `reply`
(a path; the item is gone once the path is). An event with a non-empty string
`key` replaces any queued item of the same type and key (a status re-sent every
turn).
"""

import os
import secrets
import threading
from collections.abc import Iterable
from dataclasses import dataclass

from .data import Obj
from .event import Event

CAP = 200


def matches(prefixes: Iterable[str], type_: str) -> bool:
    return any(type_ == p or type_.startswith(p + ".") for p in prefixes)


@dataclass(frozen=True)
class Item:
    id: str
    received: float
    event: Event

    def to_json(self) -> Obj:
        return {"id": self.id, "received": self.received, "event": self.event.data}


def expired(item: Item, now: float) -> bool:
    data = item.event.data
    timeout = data.get("timeout")
    if isinstance(timeout, int | float) and not isinstance(timeout, bool):
        if now > item.received + timeout:
            return True
    reply = data.get("reply")
    return isinstance(reply, str) and reply != "" and not os.path.lexists(reply)


def replaces(new: Event, old: Event) -> bool:
    key = new.data.get("key")
    return isinstance(key, str) and key != "" and new.type == old.type and old.data.get("key") == key


class Inbox:
    """Thread-safe: the HTTP server calls it from request threads."""

    def __init__(self, prefixes: Iterable[str]) -> None:
        self.prefixes = list(prefixes)
        self._items: list[Item] = []
        self._lock = threading.Lock()

    def _prune(self, now: float) -> None:
        self._items = [i for i in self._items if not expired(i, now)]

    def add(self, event: Event, now: float) -> str | None:
        """Queue the event; its id, or None if its type is not one of the prefixes."""
        if not matches(self.prefixes, event.type):
            return None
        item = Item(secrets.token_hex(8), now, event)
        with self._lock:
            self._prune(now)
            self._items = [i for i in self._items if not replaces(event, i.event)]
            self._items.append(item)
            del self._items[:-CAP]
        return item.id

    def items(self, now: float) -> list[Item]:
        """Oldest first."""
        with self._lock:
            self._prune(now)
            return list(self._items)

    def remove(self, id_: str) -> bool:
        with self._lock:
            kept = [i for i in self._items if i.id != id_]
            removed = len(kept) != len(self._items)
            self._items = kept
            return removed
