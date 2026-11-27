"""How to run a signed command."""

import sys
from typing import Protocol

from .data import Obj, dumps, get_str
from .logic import sanitize
from .signed import Command

MAX_AGE = 300  # seconds


class BusError(Exception):
    """Publishing on the message bus failed."""


class Bus(Protocol):
    def publish(self, subject: str, data: str) -> None:
        """BusError on failure."""
        ...


def _notice(bus: Bus, event_id: str, text: str) -> None:
    bus.publish("matrix.send", dumps({"reply_to": event_id, "body": text}))


def _failed(reason: str) -> str:
    return "Failed: " + sanitize(reason)[:300]


class Seen(Protocol):
    def claim(self, event_id: str) -> bool:
        """True the first time; OSError propagates."""
        ...


class Tasks(Protocol):
    def start(self, prompt: str) -> Obj | None:
        """The event event-handler-task printed, None if none."""
        ...


def command(
    cmd: Command, host: str, now: float, seen: Seen, tasks: Tasks, bus: Bus
) -> None:
    if cmd.to != host:
        print(
            "matrix-inbox-command: ignoring a command for another host", file=sys.stderr
        )
        return
    if abs(now - cmd.ts / 1000) > MAX_AGE:
        print(
            "matrix-inbox-command: ignoring a stale or future-dated command",
            file=sys.stderr,
        )
        return
    if not seen.claim(cmd.event_id):
        print(
            "matrix-inbox-command: ignoring an already handled command", file=sys.stderr
        )
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
