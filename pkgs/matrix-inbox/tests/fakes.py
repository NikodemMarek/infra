from dataclasses import dataclass

from matrix_inbox.data import Json, Obj

USER = "@agents:nkmrk.com"
OWNER = "@nikodem:nkmrk.com"
ROOM = "!room:nkmrk.com"
META = "com.nkmrk.inbox"
PERMISSION = "claude.permission_request"
QUESTION = "claude.question"


@dataclass(frozen=True)
class Sent:
    room: str
    content: Obj
    txn: str
    event_type: str
    event_id: str


class FakeMatrix:
    """The `Matrix` of the tests: records sends, replays scripted syncs, serves events it was given or sent."""

    def __init__(self, user: str = USER, aliases: dict[str, str] | None = None) -> None:
        self.user = user
        self.aliases: dict[str, str] = dict(aliases) if aliases is not None else {}
        self.created: list[tuple[str, str]] = []
        self.created_room = "!created:nkmrk.com"
        self.recent: list[Obj] = []
        self.recent_calls: list[tuple[str, str, int]] = []
        self.resolved: list[str] = []
        self.script: list[tuple[str, list[Obj]]] = []
        self.sync_calls: list[tuple[str, str | None, int]] = []
        self.sync_error: Exception | None = None
        self.events: dict[str, Obj] = {}
        self.sent: list[Sent] = []
        self.send_errors: dict[str, Exception] = {}

    def whoami(self) -> str:
        return self.user

    def resolve_alias(self, alias: str) -> str | None:
        self.resolved.append(alias)
        return self.aliases.get(alias)

    def create_room(self, alias_localpart: str, invite: str) -> str:
        self.created.append((alias_localpart, invite))
        return self.created_room

    def recent_messages(self, room: str, sender: str, limit: int) -> list[Obj]:
        self.recent_calls.append((room, sender, limit))
        return list(self.recent)

    def sync(self, room: str, since: str | None, timeout_ms: int) -> tuple[str, list[Obj]]:
        self.sync_calls.append((room, since, timeout_ms))
        if self.sync_error is not None:
            raise self.sync_error
        if self.script:
            return self.script.pop(0)
        return (f"s{len(self.sync_calls)}", [])

    def get_event(self, room: str, event_id: str) -> Obj | None:
        return self.events.get(event_id)

    def send(self, room: str, content: Obj, txn: str, event_type: str = "m.room.message") -> str:
        error = self.send_errors.get(txn)
        if error is not None:
            raise error
        event_id = f"$sent{len(self.sent) + 1}"
        self.sent.append(Sent(room, content, txn, event_type, event_id))
        self.events[event_id] = {"event_id": event_id, "type": event_type, "sender": self.user, "content": content}
        return event_id

    def add_event(self, event_id: str, sender: str, content: Obj) -> None:
        self.events[event_id] = {"event_id": event_id, "type": "m.room.message", "sender": sender, "content": content}

    def notices(self) -> list[Sent]:
        return [s for s in self.sent if s.content.get("msgtype") == "m.notice"]

    def reactions(self) -> list[Sent]:
        return [s for s in self.sent if s.event_type == "m.reaction"]

    def posts(self) -> list[Sent]:
        return [s for s in self.sent if META in s.content]


class FakeLocal:
    """The `Local` of the tests: holds the inbox items, records replies."""

    def __init__(self, items: list[Obj] | None = None) -> None:
        self.held: list[Obj] = list(items) if items is not None else []
        self.items_error: Exception | None = None
        self.replies: list[tuple[str, str]] = []
        self.reply_results: dict[str, str | None] = {}

    def items(self) -> list[Obj]:
        if self.items_error is not None:
            raise self.items_error
        return list(self.held)

    def reply(self, item_id: str, line: str) -> str | None:
        self.replies.append((item_id, line))
        return self.reply_results.get(item_id)


def item(item_id: str, event: Obj, received: float = 1.0) -> Obj:
    return {"id": item_id, "received": received, "event": event}


def permission(reply: str | None = "/r/p", command: str = "ls -la", **extra: Json) -> Obj:
    event: Obj = {
        "type": PERMISSION,
        "project": "infra",
        "session": "s-1",
        "title": "List files",
        "tool_name": "Bash",
        "tool_input": {"command": command},
    }
    if reply is not None:
        event["reply"] = reply
    event.update(extra)
    return event


def question(reply: str | None = "/r/q", multi: bool = False, **extra: Json) -> Obj:
    q: Obj = {"question": "Which db?", "options": [{"label": "A"}, {"label": "B", "description": "fast"}, {"label": "C"}]}
    if multi:
        q["multiSelect"] = True
    event: Obj = {"type": QUESTION, "project": "infra", "tool_input": {"questions": [q]}}
    if reply is not None:
        event["reply"] = reply
    event.update(extra)
    return event


def bot_message(item_id: str, kind: str) -> Obj:
    return {"msgtype": "m.text", "body": "posted", META: {"id": item_id, "kind": kind}}


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
