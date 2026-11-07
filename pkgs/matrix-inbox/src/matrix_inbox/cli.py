"""`matrix-inbox-post` and `matrix-inbox-answer`: event-handlers entry points."""

import os
import socket
import sys
from typing import NoReturn

from .data import ParseError, loads
from .handlers import BusError, answer as answer_reply, post as post_event
from .io import NatsBus, ProcessLocal
from .signed import load_key, open_reply


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


def post() -> None:
    _no_args("matrix-inbox-post", "the event JSON object")
    try:
        event = loads(_read_stdin())
    except ParseError as e:
        _fail(f"matrix-inbox-post: {e}", 1)
    if not isinstance(event, dict):
        _fail("matrix-inbox-post: stdin is not a JSON object", 1)
    try:
        post_event(event, _host(), _bus())
    except BusError as e:
        _fail(f"matrix-inbox-post: {e}", 1)


def answer() -> None:
    _no_args("matrix-inbox-answer", "the signed reply line")
    try:
        key = load_key(os.environ.get("MATRIX_GATEWAY_KEY", ""))
    except ValueError:
        _fail("matrix-inbox-answer: set MATRIX_GATEWAY_KEY to the gateway's public key", 2)
    reply = open_reply(_read_stdin().decode("utf-8", errors="replace"), key)
    if reply is None:
        _fail("matrix-inbox-answer: dropping an unsigned or invalid reply", 1)
    try:
        answer_reply(reply, _host(), ProcessLocal(), _bus())
    except BusError as e:
        _fail(f"matrix-inbox-answer: {e}", 1)
