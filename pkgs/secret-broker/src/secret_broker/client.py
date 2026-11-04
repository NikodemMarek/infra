"""`secret`: the agent-side client of the broker socket."""

import os
import socket
import sys
from pathlib import Path
from typing import TextIO

from . import data
from .data import Obj, ParseError
from .protocol import encode
from .text import clean


DEFAULT_SOCKET = "/run/secret-broker.sock"
CONSOLE = "(Alt+SysRq+R, Ctrl+Alt+F9)"


def _eprint(msg: str) -> None:
    print(msg, file=sys.stderr)


def _write(stream: TextIO, b: bytes) -> None:
    # raw bytes past the text layer (sys.std*.buffer is typed as Any)
    stream.flush()
    view = memoryview(b)
    while view:
        view = view[os.write(stream.fileno(), view) :]


def _final(op: str, resp: Obj) -> int:
    """Print the broker's final answer; the exit code."""
    reason = clean(data.opt_str(resp, "reason", "") or "")
    match data.get_str(resp, "status"):
        case "denied":
            _eprint(f"secret: denied: {reason}")
            return 77
        case "ok":
            pass
        case _:
            _eprint(f"secret: error: {reason}")
            return 70
    match op:
        case "list":
            for a in data.as_list(resp.get("actions"), "actions"):
                o = data.as_obj(a, "action")
                detail = data.get_str(o, "detail")
                print(
                    f"{data.get_str(o, 'name')}\t{data.get_str(o, 'state')}{' ' + detail if detail else ''}"
                    f"\t{data.get_str(o, 'description')}\t{' '.join(data.str_list(o.get('args'), 'args'))}"
                )
            return 0
        case "run":
            try:
                _write(sys.stdout, data.unb64(data.get_str(resp, "stdout")))
                _write(sys.stderr, data.unb64(data.get_str(resp, "stderr")))
            except BrokenPipeError:
                return 141  # 128 + SIGPIPE
            if data.opt_bool(resp, "truncated", False):
                _eprint("secret: output truncated")
            return data.opt_int(resp, "exit", 0)
        case _:
            print(data.dumps(data.as_obj(resp.get("profiles"), "profiles")))
            return 0


def _exchange(path: str, req: Obj) -> int:
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        s.connect(path)
    except OSError:
        s.close()
        _eprint(f"secret: broker not reachable ({path})")
        return 69
    op = data.get_str(req, "op")
    try:
        with s, s.makefile("rb") as f:
            s.sendall(encode(req))
            for line in f:
                resp = data.as_obj(data.loads(line), "reply")
                if data.get_str(resp, "status") != "pending":
                    return _final(op, resp)
                tag = data.opt_str(resp, "tag", None)
                if tag is None:
                    _eprint(f"secret: waiting for approval on the console {CONSOLE}…")
                else:
                    _eprint(f"secret: approve on the console only if it shows tag {tag} {CONSOLE}")
    except ParseError:
        _eprint("secret: error: bad reply from the broker")
        return 70
    except OSError:
        pass
    _eprint("secret: broker closed the connection")
    return 70


def main(argv: list[str] | None = None) -> int:
    """`secret list | run ACTION [ARG...] | export PROFILE...`.

    Exit codes: 64 usage, 69 connect failed, 70 error status or EOF before the final line,
    77 denied, else the action's exit.
    """
    args = sys.argv[1:] if argv is None else argv
    match args:
        case ["list"]:
            req: Obj = {"op": "list"}
        case ["run", action, *rest]:
            req = {
                "op": "run",
                "action": action,
                "args": [*rest],
                "cwd": os.getcwd(),
                "session": os.environ.get("SHPOOL_SESSION_NAME", ""),
            }
        case ["export", *profiles] if profiles:
            try:
                spec = Path("secretspec.toml").read_text(encoding="utf-8")
            except (OSError, UnicodeDecodeError):
                _eprint("secret: cannot read ./secretspec.toml")
                return 64
            req = {
                "op": "export",
                "profiles": [*profiles],
                "spec": spec,
                "cwd": os.getcwd(),
                "session": os.environ.get("SHPOOL_SESSION_NAME", ""),
            }
        case _:
            _eprint("usage: secret list | run ACTION [ARG...] | export PROFILE...")
            return 64
    return _exchange(os.environ.get("SECRET_BROKER_SOCKET") or DEFAULT_SOCKET, req)
