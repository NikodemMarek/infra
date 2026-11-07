from dataclasses import dataclass

from matrix_gateway.data import Obj
from matrix_gateway.gateway import BusError

USER = "@agents:nkmrk.com"
OWNER = "@nikodem:nkmrk.com"
ROOM = "!room:nkmrk.com"


@dataclass(frozen=True)
class Sent:
    room: str
    content: Obj
    txn: str
    event_type: str
    event_id: str


class FakeMatrix:
    def __init__(self, user: str = USER, aliases: dict[str, str] | None = None) -> None:
        self.user = user
        self.aliases: dict[str, str] = dict(aliases) if aliases is not None else {}
        self.logins: list[tuple[str, str, str]] = []
        self.created: list[tuple[str, str]] = []
        self.created_room = "!created:nkmrk.com"
        self.resolved: list[str] = []
        self.script: list[tuple[str, list[Obj]]] = []
        self.sync_calls: list[tuple[str, str | None, int]] = []
        self.events: dict[str, Obj] = {}
        self.event_errors: dict[str, Exception] = {}
        self.sent: list[Sent] = []
        self.send_attempts: list[str] = []
        self.send_failures: list[Exception] = []

    def login(self, user: str, password: str, device_id: str) -> str:
        self.logins.append((user, password, device_id))
        return self.user

    def resolve_alias(self, alias: str) -> str | None:
        self.resolved.append(alias)
        return self.aliases.get(alias)

    def create_room(self, alias_localpart: str, invite: str) -> str:
        self.created.append((alias_localpart, invite))
        return self.created_room

    def sync(self, room: str, since: str | None, timeout_ms: int) -> tuple[str, list[Obj]]:
        self.sync_calls.append((room, since, timeout_ms))
        if self.script:
            return self.script.pop(0)
        return (f"s{len(self.sync_calls)}", [])

    def get_event(self, room: str, event_id: str) -> Obj | None:
        error = self.event_errors.get(event_id)
        if error is not None:
            raise error
        return self.events.get(event_id)

    def send(self, room: str, content: Obj, txn: str, event_type: str = "m.room.message") -> str:
        self.send_attempts.append(txn)
        if self.send_failures:
            raise self.send_failures.pop(0)
        event_id = f"$sent{len(self.sent) + 1}"
        self.sent.append(Sent(room, content, txn, event_type, event_id))
        return event_id

    def add_event(self, event_id: str, sender: str, content: Obj) -> None:
        self.events[event_id] = {"event_id": event_id, "type": "m.room.message", "sender": sender, "content": content}

    def notices(self) -> list[Sent]:
        return [s for s in self.sent if s.content.get("msgtype") == "m.notice"]


class FakeBus:
    def __init__(self) -> None:
        self.published: list[tuple[str, str]] = []
        self.error: Exception | None = None

    def publish(self, subject: str, data: str) -> None:
        if self.error is not None:
            raise self.error
        self.published.append((subject, data))

    def fail(self) -> None:
        self.error = BusError("down")


def owner_message(
    event_id: str,
    body: str,
    parent: str | None = None,
    sender: str = OWNER,
    msgtype: str = "m.text",
    rel_type: str | None = None,
    event_type: str = "m.room.message",
) -> Obj:
    relates: Obj = {}
    if parent is not None:
        relates["m.in_reply_to"] = {"event_id": parent}
    if rel_type is not None:
        relates["rel_type"] = rel_type
        relates["event_id"] = "$orig"
    content: Obj = {"msgtype": msgtype, "body": body}
    if relates:
        content["m.relates_to"] = relates
    return {"type": event_type, "sender": sender, "event_id": event_id, "content": content}
