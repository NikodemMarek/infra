"""The bot loop: mirror offered inbox items into the room, answer the owner's replies."""

import logging
from typing import Protocol

from .data import Obj, get_obj, get_str
from .logic import (
    ANSWERABLE,
    META,
    Refusal,
    claimant,
    detail,
    offered,
    parse_permission,
    parse_question,
    render,
    sanitize,
    strip_fallback,
)

log = logging.getLogger("matrix_inbox")

NOTICE_NOT_A_REPLY = "Reply to an item to answer it."
NOTICE_GONE = "No longer waiting: answered elsewhere or expired."
NOTICE_CLAIMED = "Refusing: another item claims the same reply target."
NOTICE_UNREADABLE = "Failed: cannot read the inbox."


class MatrixError(Exception):
    """Transient (incl. HTTP 429): logged, retried next round."""


class LocalError(Exception):
    """agent-inbox list failed."""


class Fatal(Exception):
    """Exit 1 (HTTP 401)."""


class Matrix(Protocol):
    def whoami(self) -> str: ...

    def resolve_alias(self, alias: str) -> str | None: ...

    def create_room(self, alias_localpart: str, invite: str) -> str: ...

    def recent_messages(self, room: str, sender: str, limit: int) -> list[Obj]:
        """m.room.message events, newest first."""
        ...

    def sync(self, room: str, since: str | None, timeout_ms: int) -> tuple[str, list[Obj]]:
        """next_batch, timeline events of room."""
        ...

    def get_event(self, room: str, event_id: str) -> Obj | None: ...

    def send(self, room: str, content: Obj, txn: str, event_type: str = "m.room.message") -> str: ...


class Local(Protocol):
    def items(self) -> list[Obj]:
        """`agent-inbox list --json`; LocalError on failure."""
        ...

    def reply(self, item_id: str, line: str) -> str | None:
        """`agent-inbox reply <id>`; None = ok, else error text."""
        ...


class Bot:
    def __init__(self, matrix: Matrix, local: Local, owner: str) -> None:
        self._matrix = matrix
        self._local = local
        self._owner = owner
        self._user = ""
        self._room = ""
        self._since: str | None = None
        self._posted: set[str] = set()

    def start(self) -> None:
        self._user = self._matrix.whoami()
        alias = "#agents:" + self._user.partition(":")[2]
        self._room = self._matrix.resolve_alias(alias) or self._matrix.create_room("agents", self._owner)
        for event in self._matrix.recent_messages(self._room, self._user, 100):
            meta = get_obj(get_obj(event, "content") or {}, META)
            item_id = get_str(meta, "id") if meta is not None else None
            if item_id is not None:
                self._posted.add(item_id)
        self._since = self._matrix.sync(self._room, None, 0)[0]

    def step(self) -> None:
        self._since, events = self._matrix.sync(self._room, self._since, 5000)
        for event in events:
            try:
                self.on_event(event)
            except (MatrixError, LocalError) as e:
                log.warning("handling an event failed: %s", e)
        self._mirror()

    def _mirror(self) -> None:
        items = self._local.items()
        for item in items:
            item_id = get_str(item, "id")
            if item_id is None or item_id in self._posted or not offered(item):
                continue
            if claimant(items, get_str(get_obj(item, "event") or {}, "reply") or "") != item_id:
                continue
            try:
                self._matrix.send(self._room, render(item), "inbox-" + item_id)
            except MatrixError as e:
                log.warning("posting %s failed: %s", item_id, e)
                continue
            self._posted.add(item_id)
        self._posted &= {i for i in (get_str(item, "id") for item in items) if i is not None}

    def on_event(self, ev: Obj) -> None:
        content = get_obj(ev, "content") or {}
        event_id = get_str(ev, "event_id")
        relates = get_obj(content, "m.relates_to") or {}
        if (
            event_id is None
            or get_str(ev, "type") != "m.room.message"
            or get_str(ev, "sender") != self._owner
            or get_str(content, "msgtype") != "m.text"
            or get_str(relates, "rel_type") == "m.replace"
        ):
            return
        notice = self._decide(content, relates)
        if notice is not None:
            self._matrix.send(
                self._room,
                {"msgtype": "m.notice", "body": notice, "m.relates_to": {"m.in_reply_to": {"event_id": event_id}}},
                "re-" + event_id,
            )
            return
        reaction: Obj = {"m.relates_to": {"rel_type": "m.annotation", "event_id": event_id, "key": "✅"}}
        self._matrix.send(self._room, reaction, "ok-" + event_id, "m.reaction")

    def _decide(self, content: Obj, relates: Obj) -> str | None:
        """The notice to answer with, or None when the item was answered."""
        parent_id = get_str(get_obj(relates, "m.in_reply_to") or {}, "event_id")
        parent = self._matrix.get_event(self._room, parent_id) if parent_id is not None else None
        if parent is None or get_str(parent, "sender") != self._user:
            return NOTICE_NOT_A_REPLY
        meta = get_obj(get_obj(parent, "content") or {}, META) or {}
        item_id = get_str(meta, "id")
        kind = get_str(meta, "kind")
        if item_id is None or kind not in ANSWERABLE:
            return NOTICE_NOT_A_REPLY
        try:
            items = self._local.items()
        except LocalError:
            return NOTICE_UNREADABLE
        item = next((i for i in items if get_str(i, "id") == item_id), None)
        event = get_obj(item, "event") if item is not None else None
        if item is None or event is None or get_str(event, "type") != kind or not offered(item):
            return NOTICE_GONE
        if claimant(items, get_str(event, "reply") or "") != item_id:
            return NOTICE_CLAIMED
        text = strip_fallback(get_str(content, "body") or "").strip()
        if kind == "claude.permission_request":
            line = parse_permission(text, detail(event)[1])
        else:
            line = parse_question(get_obj(event, "tool_input") or {}, text)
        if isinstance(line, Refusal):
            return line.reason
        err = self._local.reply(item_id, line)
        return None if err is None else "Failed: " + sanitize(err, single_line=True)[:300]
