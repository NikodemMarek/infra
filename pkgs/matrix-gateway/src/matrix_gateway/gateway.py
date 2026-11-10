"""The relay: Matrix room events to NATS replies and NATS requests to Matrix posts."""

import logging
from typing import Protocol

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

from .data import Obj, get_int, get_obj, get_str
from .logic import Send, content, parse_command, parse_send, route, seal, seal_command, strip_fallback

DEVICE_ID = "matrix-gateway"

NOTICE_NOT_A_REPLY = "Reply to a message that asks for an answer, or send !<host> <task> as a new message."
NOTICE_BUS = "Failed: message bus unavailable."

log = logging.getLogger("matrix_gateway")


class MatrixError(Exception):
    """Transient (incl. 429)."""


class Fatal(Exception):
    """HTTP 401/403: the caller exits 1."""


class BusError(Exception):
    pass


class Matrix(Protocol):
    def login(self, user: str, password: str, device_id: str) -> str: ...
    def resolve_alias(self, alias: str) -> str | None: ...
    def create_room(self, alias_localpart: str, invite: str) -> str: ...
    def sync(self, room: str, since: str | None, timeout_ms: int) -> tuple[str, list[Obj]]: ...
    def get_event(self, room: str, event_id: str) -> Obj | None: ...
    def send(self, room: str, content: Obj, txn: str, event_type: str = "m.room.message") -> str: ...


class Bus(Protocol):
    def publish(self, subject: str, data: str) -> None: ...


class Gateway:
    def __init__(self, matrix: Matrix, bus: Bus, owner: str, user: str, password: str, key: Ed25519PrivateKey) -> None:
        self._matrix = matrix
        self._bus = bus
        self._owner = owner
        self._user = user
        self._password = password
        self._key = key
        self._bot = ""
        self._room = ""
        self._since: str | None = None

    def start(self) -> None:
        self._bot = self._matrix.login(self._user, self._password, DEVICE_ID)
        alias = "#agents:" + self._bot.partition(":")[2]
        self._room = self._matrix.resolve_alias(alias) or self._matrix.create_room("agents", self._owner)
        self._since = self._matrix.sync(self._room, None, 0)[0]

    def step(self) -> None:
        self._since, events = self._matrix.sync(self._room, self._since, 30000)
        for event in events:
            try:
                self.on_event(event)
            except MatrixError as e:
                log.warning("handling an event failed: %s", e)

    def on_event(self, ev: Obj) -> None:
        body = get_obj(ev, "content") or {}
        event_id = get_str(ev, "event_id")
        relates = get_obj(body, "m.relates_to") or {}
        if (
            event_id is None
            or get_str(ev, "type") != "m.room.message"
            or get_str(ev, "sender") != self._owner
            or get_str(body, "msgtype") != "m.text"
            or get_str(relates, "rel_type") == "m.replace"
        ):
            return
        parent_id = get_str(get_obj(relates, "m.in_reply_to") or {}, "event_id")
        if parent_id is None:
            self._on_top_level(ev, event_id, get_str(body, "body") or "")
            return
        parent = self._matrix.get_event(self._room, parent_id)
        target = route(parent, self._bot) if parent is not None else None
        if parent is None or target is None:
            self._notice(event_id, NOTICE_NOT_A_REPLY)
            return
        to, ref = target
        text = strip_fallback(get_str(body, "body") or "").strip()
        parent_body = get_str(get_obj(parent, "content") or {}, "body") or ""
        try:
            self._bus.publish("matrix.reply." + to, seal(self._key, to, ref, text, parent_body, event_id))
        except BusError as e:
            log.warning("publishing a reply failed: %s", e)
            self._notice(event_id, NOTICE_BUS)

    def _on_top_level(self, ev: Obj, event_id: str, body: str) -> None:
        command = parse_command(body)
        if command is None:
            self._notice(event_id, NOTICE_NOT_A_REPLY)
            return
        ts = get_int(ev, "origin_server_ts")
        if ts is None:
            log.warning("dropping a command without origin_server_ts")
            return
        to, text = command
        try:
            self._bus.publish("matrix.command." + to, seal_command(self._key, to, text, event_id, ts))
        except BusError as e:
            log.warning("publishing a command failed: %s", e)
            self._notice(event_id, NOTICE_BUS)

    def on_send(self, line: str, txn: str) -> None:
        send = parse_send(line)
        if send is None:
            log.warning("dropping an invalid matrix.send message")
            return
        self._matrix.send(self._room, content(send), txn)

    def _notice(self, event_id: str, text: str) -> None:
        self._matrix.send(self._room, content(Send(text, reply_to=event_id)), "re-" + event_id)
