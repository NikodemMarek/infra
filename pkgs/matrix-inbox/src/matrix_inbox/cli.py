"""`matrix-inbox-command`: the event-handlers entry point."""

import os
import socket
import sys
import time
from typing import NoReturn

from .handlers import (
    BusError,
)
from .handlers import (
    command as run_command,
)
from .io import NatsBus, ProcessTasks, SeenDir
from .signed import load_key, open_command


def _fail(message: str, code: int) -> NoReturn:
    print(message, file=sys.stderr)
    sys.exit(code)


def _no_args(name: str, what: str) -> None:
    if len(sys.argv) > 1:
        _fail(f"usage: {name} ({what} on stdin)", 2)


def _bus() -> NatsBus:
    return NatsBus(os.environ.get("NATS_URL") or "nats://127.0.0.1:4222")


def _host() -> str:
    return socket.gethostname().split(".")[0]


def _read_stdin() -> bytes:
    with open(0, "rb", closefd=False) as f:
        return f.read()


def command() -> None:
    _no_args("matrix-inbox-command", "the signed command line")
    try:
        key = load_key(os.environ.get("MATRIX_GATEWAY_KEY", ""))
    except ValueError:
        _fail(
            "matrix-inbox-command: set MATRIX_GATEWAY_KEY to the gateway's public key",
            2,
        )
    cmd = open_command(_read_stdin().decode("utf-8", errors="replace"), key)
    if cmd is None:
        _fail("matrix-inbox-command: dropping an unsigned or invalid command", 1)
    runtime_dir = os.environ.get("XDG_RUNTIME_DIR") or f"/run/user/{os.getuid()}"
    try:
        run_command(
            cmd,
            _host(),
            time.time(),
            SeenDir(runtime_dir + "/matrix-inbox/commands"),
            ProcessTasks(),
            _bus(),
        )
    except (BusError, OSError) as e:
        _fail(f"matrix-inbox-command: {e}", 1)
