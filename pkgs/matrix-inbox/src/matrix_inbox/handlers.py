"""What to publish for an offered event, how to answer a signed reply and how to run a signed command."""

import sys
from typing import Protocol

from .data import Obj, dumps, get_obj, get_str
from .logic import Refusal, claimant, detail, offered, parse_permission, parse_question, render, sanitize
from .signed import Command, Reply

MAX_AGE = 300  # seconds


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


def _notice(bus: Bus, event_id: str, text: str) -> None:
    bus.publish("matrix.send", dumps({"reply_to": event_id, "body": text}))


def _failed(reason: str) -> str:
    return "Failed: " + sanitize(reason, single_line=True)[:300]


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
        _notice(bus, reply.event_id, "Failed: cannot read the inbox.")
        return
    waiting = _waiting(items, reply.ref)
    if waiting is None:
        _notice(bus, reply.event_id, "No longer waiting: answered elsewhere or expired.")
        return
    item_id, event = waiting
    if render(event) != reply.body:
        _notice(bus, reply.event_id, "Refusing: the message does not match the waiting request.")
        return
    text = reply.text.strip()
    if get_str(event, "type") == "claude.permission_request":
        line = parse_permission(text, detail(event)[1])
    else:
        line = parse_question(get_obj(event, "tool_input") or {}, text)
    if isinstance(line, Refusal):
        _notice(bus, reply.event_id, line.reason)
        return
    error = local.reply(item_id, line)
    _notice(bus, reply.event_id, "✅ Answered." if error is None else _failed(error))


class Seen(Protocol):
    def claim(self, event_id: str) -> bool:
        """True the first time; OSError propagates."""
        ...


class Tasks(Protocol):
    def start(self, prompt: str) -> Obj | None:
        """The event event-handler-task printed, None if none."""
        ...


def command(cmd: Command, host: str, now: float, seen: Seen, tasks: Tasks, bus: Bus) -> None:
    if cmd.to != host:
        print("matrix-inbox-command: ignoring a command for another host", file=sys.stderr)
        return
    if abs(now - cmd.ts / 1000) > MAX_AGE:
        print("matrix-inbox-command: ignoring a stale or future-dated command", file=sys.stderr)
        return
    if not seen.claim(cmd.event_id):
        print("matrix-inbox-command: ignoring an already handled command", file=sys.stderr)
        return
    outcome = tasks.start(cmd.text) or {}
    kind = get_str(outcome, "type")
    if kind == "task.started":
        body = f"Started {get_str(outcome, 'shpool') or ''} in {get_str(outcome, 'project') or ''}."
    elif kind == "task.failed":
        reason = get_str(outcome, "reason")
        body = _failed("unknown" if reason is None else reason)
    else:
        body = "Failed: the task starter gave no result."
    _notice(bus, cmd.event_id, body)
