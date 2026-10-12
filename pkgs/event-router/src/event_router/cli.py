"""Receive events over HTTP and dispatch them to handlers."""

import argparse
import logging
import os
import sys
import urllib.error
import urllib.request
from pathlib import Path

from .data import Obj, ParseError, as_obj, loads, read_stdin
from .event import Event
from .handlers import Dispatcher
from .inbox import Inbox
from .server import serve

log = logging.getLogger("event-router")

DEFAULT_PORT = 7531
TIMEOUT = 10


class Args(argparse.Namespace):
    command: str
    type: str
    verbose: bool


def _parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog="event-router", description=__doc__)
    p.add_argument("-v", "--verbose", action="store_true")
    sub = p.add_subparsers(dest="command", required=True)
    sub.add_parser("serve", help="listen for events, dispatching handlers")
    emit = sub.add_parser("emit", help="send an event to the router; a JSON object on stdin is merged in")
    emit.add_argument("type", help="e.g. gitlab.note")
    return p


def _port() -> int:
    v = os.environ.get("EVENT_ROUTER_PORT")
    return int(v) if v else DEFAULT_PORT


def _url() -> str:
    return os.environ.get("EVENT_ROUTER_URL") or f"http://127.0.0.1:{_port()}"


def _emit(type_: str) -> None:
    raw = read_stdin()
    payload: Obj = as_obj(loads(raw), "stdin") if raw.strip() else {}
    event = Event.from_json({**payload, "type": type_})
    req = urllib.request.Request(
        f"{_url()}/events",
        data=event.dumps().encode(),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=TIMEOUT):
        pass


def main() -> None:
    args = _parser().parse_args(namespace=Args())
    logging.basicConfig(
        level=logging.DEBUG if args.verbose else logging.INFO,
        format="%(levelname)s %(message)s",
        stream=sys.stderr,
    )

    if args.command == "emit":
        try:
            _emit(args.type)
        except ParseError as e:
            log.error("%s", e)
            sys.exit(2)
        except urllib.error.HTTPError as e:
            log.error("router: %d %s", e.code, e.read().decode(errors="replace").strip())
            sys.exit(1)
        except (urllib.error.URLError, OSError) as e:
            log.error("router %s: %s", _url(), e)
            sys.exit(1)
        return

    handlers = os.environ.get("EVENT_ROUTER_HANDLERS")
    try:
        inbox = Inbox(os.environ.get("EVENT_ROUTER_INBOX", "").split())
        serve(_port(), Dispatcher(Path(handlers) if handlers else None), inbox)
    except KeyboardInterrupt:
        pass
