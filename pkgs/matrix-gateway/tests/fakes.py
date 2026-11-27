from dataclasses import dataclass

from matrix_gateway.data import Obj
from matrix_gateway.gateway import BusError

USER = "@agents:example.com"
OWNER = "@nikodem:example.com"
ROOM = "!room:example.com"
MAIL_ROOM = "!mail:example.com"
ALERTS_ROOM = "!alerts:example.com"


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
        self.resolved: list[str] = []
        self.script: list[tuple[str, list[Obj]]] = []
        self.sync_calls: list[tuple[str, str | None, int]] = []
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
        return f"!{alias_localpart}:example.com"

    def sync(
        self, room: str, since: str | None, timeout_ms: int
    ) -> tuple[str, list[Obj]]:
        self.sync_calls.append((room, since, timeout_ms))
        if self.script:
            return self.script.pop(0)
        return (f"s{len(self.sync_calls)}", [])

    def send(
        self, room: str, content: Obj, txn: str, event_type: str = "m.room.message"
    ) -> str:
        self.send_attempts.append(txn)
        if self.send_failures:
            raise self.send_failures.pop(0)
        event_id = f"$sent{len(self.sent) + 1}"
        self.sent.append(Sent(room, content, txn, event_type, event_id))
        return event_id


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
    sender: str = OWNER,
    msgtype: str = "m.text",
    rel_type: str | None = None,
    event_type: str = "m.room.message",
) -> Obj:
    relates: Obj = {}
    if rel_type is not None:
        relates["rel_type"] = rel_type
        relates["event_id"] = "$orig"
    content: Obj = {"msgtype": msgtype, "body": body}
    if relates:
        content["m.relates_to"] = relates
    return {
        "type": event_type,
        "sender": sender,
        "event_id": event_id,
        "content": content,
    }
