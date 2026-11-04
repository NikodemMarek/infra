"""Run the handler for an event: `<dir>/<type>`, then its prefixes (`a.b.c`, `a.b`, `a`), then `default`."""

import logging
import os
import subprocess
import tempfile
import threading
from dataclasses import dataclass
from pathlib import Path

from .event import Event

log = logging.getLogger(__name__)


@dataclass
class Running:
    proc: subprocess.Popen[bytes]
    handler: Path
    event: str


def find(handlers_dir: Path | None, event: Event) -> Path | None:
    if handlers_dir is None:
        return None
    parts = event.type.split(".")
    for name in [".".join(parts[:i]) for i in range(len(parts), 0, -1)] + ["default"]:
        p = handlers_dir / name
        if _runnable(p):
            return p
    return None


def _runnable(p: Path) -> bool:
    return p.is_file() and os.access(p, os.X_OK)


def _env(event: Event) -> dict[str, str]:
    return dict(os.environ, ER_TYPE=event.type)


class Dispatcher:
    """Thread-safe: the HTTP server dispatches from request threads."""

    def __init__(self, handlers_dir: Path | None) -> None:
        self.handlers_dir = handlers_dir
        self.running: list[Running] = []
        self.lock = threading.Lock()

    def dispatch(self, event: Event) -> bool:
        """Start the handler; False if there is none or it can't be started."""
        handler = find(self.handlers_dir, event)
        if handler is None:
            log.warning("no handler for %s", event.describe())
            return False
        return self._start(handler, event)

    def dispatch_default(self, event: Event) -> bool:
        """Start only `<handlers_dir>/default` (a regular executable file), exactly
        like `dispatch`; never looks up type-named handlers. False if there is no
        default or it can't be started."""
        handler = self.handlers_dir / "default" if self.handlers_dir is not None else None
        if handler is None or not _runnable(handler):
            log.warning("no default handler for %s", event.describe())
            return False
        return self._start(handler, event)

    def _start(self, handler: Path, event: Event) -> bool:
        # event goes through a temp file, not a pipe: a handler that never reads
        # stdin can't block us on a full pipe buffer
        with tempfile.TemporaryFile() as stdin:
            stdin.write(event.dumps().encode())
            stdin.seek(0)
            try:
                proc = subprocess.Popen([str(handler)], stdin=stdin, env=_env(event), cwd="/")
            except OSError as e:
                log.error("%s: cannot run %s: %s", event.describe(), handler, e)
                return False
        log.info("%s -> %s", event.describe(), handler.name)
        with self.lock:
            self.running.append(Running(proc, handler, event.describe()))
        return True

    def reap(self) -> None:
        with self.lock:
            still: list[Running] = []
            for r in self.running:
                code = r.proc.poll()
                if code is None:
                    still.append(r)
                elif code != 0:
                    log.error("%s: handler %s exited with %d", r.event, r.handler.name, code)
            self.running = still
