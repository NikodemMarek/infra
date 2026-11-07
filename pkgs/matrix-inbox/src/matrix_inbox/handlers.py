"""What to publish for an offered event and how to answer a signed reply."""

import sys
from typing import Protocol

from .data import Obj, dumps, get_obj, get_str
from .logic import Refusal, claimant, detail, offered, parse_permission, parse_question, render, sanitize
from .signed import Reply


class LocalError(Exception):
    """agent-inbox list failed."""


class BusError(Exception):
    """Publishing on the message bus failed."""


class Local(Protocol):
    def items(self) -> list[Obj]:
        """`agent-inbox list --json`; LocalError on failure."""
        ...

    def reply(self, item_id: str, line: str) -> str | None:
        """`agent-inbox reply <id>`; None = ok, else error text."""
        ...


class Bus(Protocol):
    def publish(self, subject: str, data: str) -> None:
        """BusError on failure."""
        ...


def post(event: Obj, host: str, bus: Bus) -> None:
    if not offered(event):
        return
    bus.publish("matrix.send", dumps({"to": host, "ref": get_str(event, "reply"), "body": render(event)}))


def _notice(bus: Bus, reply: Reply, text: str) -> None:
    bus.publish("matrix.send", dumps({"reply_to": reply.event_id, "body": text}))


def _waiting(items: list[Obj], ref: str) -> tuple[str, Obj] | None:
    item_id = claimant(items, ref)
    if item_id is None:
        return None
    for item in items:
        event = get_obj(item, "event")
        if get_str(item, "id") == item_id and event is not None and offered(event):
            return item_id, event
    return None


def answer(reply: Reply, host: str, local: Local, bus: Bus) -> None:
    if reply.to != host:
        print("matrix-inbox-answer: ignoring a reply for another host", file=sys.stderr)
        return
    try:
        items = local.items()
    except LocalError as e:
        print(f"matrix-inbox-answer: {e}", file=sys.stderr)
        _notice(bus, reply, "Failed: cannot read the inbox.")
        return
    waiting = _waiting(items, reply.ref)
    if waiting is None:
        _notice(bus, reply, "No longer waiting: answered elsewhere or expired.")
        return
    item_id, event = waiting
    if render(event) != reply.body:
        _notice(bus, reply, "Refusing: the message does not match the waiting request.")
        return
    text = reply.text.strip()
    if get_str(event, "type") == "claude.permission_request":
        line = parse_permission(text, detail(event)[1])
    else:
        line = parse_question(get_obj(event, "tool_input") or {}, text)
    if isinstance(line, Refusal):
        _notice(bus, reply, line.reason)
        return
    error = local.reply(item_id, line)
    _notice(bus, reply, "✅ Answered." if error is None else "Failed: " + sanitize(error, single_line=True)[:300])
