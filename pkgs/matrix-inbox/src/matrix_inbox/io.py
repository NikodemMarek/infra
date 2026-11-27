"""The seen-command markers, the task starter and the NATS publisher."""

import hashlib
import os
import subprocess
import sys

from .data import Obj, ParseError, dumps, loads
from .handlers import BusError

BUS_TIMEOUT = 5
TASK_TIMEOUT = 300


class SeenDir:
    def __init__(self, path: str) -> None:
        self._path = path

    def claim(self, event_id: str) -> bool:
        os.makedirs(self._path, 0o700, exist_ok=True)
        marker = os.path.join(
            self._path, hashlib.sha256(event_id.encode("utf-8")).hexdigest()
        )
        try:
            fd = os.open(marker, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        except FileExistsError:
            return False
        os.close(fd)
        return True


class ProcessTasks:
    def start(self, prompt: str) -> Obj | None:
        try:
            done = subprocess.run(
                ["event-handler-task"],
                input=dumps({"type": "task.new", "prompt": prompt}),
                stdout=subprocess.PIPE,
                encoding="utf-8",
                errors="replace",
                timeout=TASK_TIMEOUT,
                check=False,
            )
        except (OSError, subprocess.SubprocessError) as e:
            print(f"matrix-inbox-command: {e}", file=sys.stderr)
            return None
        lines = [line for line in done.stdout.splitlines() if line.strip()]
        if not lines:
            return None
        try:
            outcome = loads(lines[-1])
        except ParseError:
            return None
        return outcome if isinstance(outcome, dict) else None


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
                check=False,
            )
        except (OSError, subprocess.SubprocessError) as e:
            raise BusError(f"nats pub: {e}") from e
        if done.returncode != 0:
            raise BusError(done.stderr.strip() or f"exit {done.returncode}")
