"""The real agent-inbox calls and the NATS publisher (subprocesses)."""

import subprocess

from .data import Obj, ParseError, loads
from .handlers import BusError, LocalError

LOCAL_TIMEOUT = 10
BUS_TIMEOUT = 5


def _agent_inbox(args: list[str], stdin: str | None = None) -> tuple[int, str, str]:
    done = subprocess.run(
        ["agent-inbox", *args],
        input=stdin,
        capture_output=True,
        encoding="utf-8",
        errors="replace",
        timeout=LOCAL_TIMEOUT,
    )
    return done.returncode, done.stdout, done.stderr.strip() or f"exit {done.returncode}"


class ProcessLocal:
    def items(self) -> list[Obj]:
        try:
            rc, out, err = _agent_inbox(["list", "--json"])
        except (OSError, subprocess.SubprocessError) as e:
            raise LocalError(f"agent-inbox list: {e}") from e
        if rc != 0:
            raise LocalError(f"agent-inbox list: {err}")
        items: list[Obj] = []
        for line in out.splitlines():
            if not line.strip():
                continue
            try:
                item = loads(line)
            except ParseError as e:
                raise LocalError(f"agent-inbox list: {e}") from e
            if not isinstance(item, dict):
                raise LocalError("agent-inbox list: line is not an object")
            items.append(item)
        return items

    def reply(self, item_id: str, line: str) -> str | None:
        try:
            rc, _, err = _agent_inbox(["reply", item_id], line + "\n")
        except (OSError, subprocess.SubprocessError) as e:
            return f"agent-inbox reply: {e}"
        return None if rc == 0 else err


class NatsBus:
    def __init__(self, url: str) -> None:
        self._url = url

    def publish(self, subject: str, data: str) -> None:
        try:
            done = subprocess.run(
                ["nats", "--server", self._url, "pub", "--no-templates", subject],
                input=data,
                capture_output=True,
                encoding="utf-8",
                errors="replace",
                timeout=BUS_TIMEOUT,
            )
        except (OSError, subprocess.SubprocessError) as e:
            raise BusError(f"nats pub: {e}") from e
        if done.returncode != 0:
            raise BusError(done.stderr.strip() or f"exit {done.returncode}")
