"""The broker service: socket activation, one thread per connection, one global lock."""

import os
import pwd
import select
import socket
import struct
import sys
import threading
import time
from collections.abc import Mapping, Sequence
from contextlib import suppress
from pathlib import Path
from typing import TextIO, cast

from . import bw as bitwarden
from . import runner, spec
from .broker import Broker
from .config import ConfigError, load_config_file
from .console import Console, disable_signal_keys, load_phrase, new_tag
from .data import Obj
from .export import ExportError, run_export
from .model import (
    Answer,
    Config,
    Denied,
    ExportReq,
    Exported,
    Failed,
    ListReq,
    Listed,
    Pending,
    Request,
    Response,
    RunReq,
    RunResult,
)
from .notify import notify
from .protocol import MAX_LINE, ProtocolError, encode, parse_request, pending_obj, response_obj
from .spec import SpecError

REQUEST_TIMEOUT = 30  # seconds for the whole request line
SEND_TIMEOUT = 30
MAX_CONNECTIONS = 8


def peer_uid(conn: socket.socket) -> int:
    """Uid of the peer, from SO_PEERCRED (struct "3i")."""
    raw = conn.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, struct.calcsize("3i"))
    _pid, uid, _gid = cast(tuple[int, int, int], struct.unpack("3i", raw))
    return uid


def listen_socket() -> socket.socket:
    """The socket passed by systemd (LISTEN_FDS == "1", LISTEN_PID == getpid, fd 3); otherwise exit 1."""
    if os.environ.get("LISTEN_FDS") != "1" or os.environ.get("LISTEN_PID") != str(os.getpid()):
        print("secret-broker: no socket from systemd (LISTEN_FDS, LISTEN_PID)", file=sys.stderr)
        raise SystemExit(1)
    return socket.socket(fileno=3)


class _ClientGone(Exception):
    """A send or receive on the connection failed."""


def _read_line(conn: socket.socket) -> bytes:
    """The request line (without its newline), read against one total deadline."""
    deadline = time.monotonic() + REQUEST_TIMEOUT
    buf = b""
    try:
        while b"\n" not in buf and len(buf) <= MAX_LINE:
            left = deadline - time.monotonic()
            if left <= 0:
                raise TimeoutError
            conn.settimeout(left)
            chunk = conn.recv(65536)
            if not chunk:
                break
            buf += chunk
        conn.settimeout(SEND_TIMEOUT)
    except OSError as e:
        raise _ClientGone from e
    return buf.partition(b"\n")[0]


def _sendall(conn: socket.socket, obj: Obj) -> None:
    try:
        conn.sendall(encode(obj))
    except OSError as e:
        raise _ClientGone from e


def _gone(conn: socket.socket) -> bool:
    """True when the peer closed its end or sent more after the request line, which it must not."""
    poller = select.poll()
    try:
        poller.register(conn, select.POLLIN)
        return bool(poller.poll(0))
    except (OSError, ValueError):
        return True


class _Cancel(threading.Event):
    """Set once the client is gone. The serving thread polls it from its wait loops, so no watcher thread is needed."""

    def __init__(self, conn: socket.socket) -> None:
        super().__init__()
        self._conn = conn

    def is_set(self) -> bool:
        if not super().is_set() and _gone(self._conn):
            self.set()
        return super().is_set()

    def wait(self, timeout: float | None = None) -> bool:
        super().wait(timeout)
        return self.is_set()


class _Ctx(threading.local):
    """The connection the current thread is serving; the broker's callbacks have no other way to see it."""

    conn: socket.socket
    cancel: threading.Event


class Server:
    def __init__(
        self,
        config: Config,
        uid: int,
        bw: str,
        secretspec: str,
        env: Mapping[str, str],
        phrase: str,
        tty_in: TextIO,
        tty_out: TextIO,
    ) -> None:
        self._config = config
        self._uid = uid
        self._bw = bw
        self._secretspec = secretspec
        self._env = env
        # serialises every action run and every bw/secretspec block
        self._lock = threading.Lock()
        # bounded by the unit's TasksMax: connections beyond this are closed at once
        self._slots = threading.BoundedSemaphore(MAX_CONNECTIONS)
        self._ctx = _Ctx()
        self._broker = Broker(config, lambda: time.clock_gettime(time.CLOCK_BOOTTIME), self._ask, self._run)
        self.console = Console(self._broker, config, bw, env, self._lock, phrase, tty_in, tty_out)

    def serve_forever(self, sock: socket.socket) -> None:
        while True:
            try:
                conn = cast(tuple[socket.socket, object], sock.accept())[0]
            except Exception as e:
                print(f"secret-broker: accept failed: {e!r}", file=sys.stderr)
                time.sleep(1)
                continue
            if not self._slots.acquire(blocking=False):
                conn.close()
                continue
            try:
                threading.Thread(target=self._slot, args=(conn,), daemon=True).start()
            except Exception as e:  # e.g. no thread to be had
                print(f"secret-broker: cannot serve a connection: {e!r}", file=sys.stderr)
                self._slots.release()
                conn.close()

    def _slot(self, conn: socket.socket) -> None:
        try:
            self._handle(conn)
        finally:
            self._slots.release()

    def _send(self, obj: Obj) -> None:
        _sendall(self._ctx.conn, obj)

    def _ask(self, pending: Pending) -> Answer:
        self._send(pending_obj(None))
        notify(f"secret request: {pending.action.name} waits on the console")
        return self.console.ask(pending, self._ctx.cancel)

    def _run(self, argv: Sequence[str], env: Mapping[str, str], timeout: int) -> RunResult:
        with self._lock:
            try:
                return runner.run_action(argv, env, timeout, self._ctx.cancel)
            finally:
                # whatever the action left behind must not outlive the lock
                if not runner.sweep(runner.own_cgroup_procs(), os.getpid()):
                    print(f"secret-broker: {runner.SWEEP_FAILED} after a run", file=sys.stderr)

    def _handle(self, conn: socket.socket) -> None:
        with conn:
            try:
                try:
                    if peer_uid(conn) != self._uid:
                        return
                except OSError:
                    return
                try:
                    req = parse_request(_read_line(conn))
                except ProtocolError:
                    _sendall(conn, response_obj(Failed("bad request")))
                    return
                self._ctx.conn, self._ctx.cancel = conn, _Cancel(conn)
                _sendall(conn, response_obj(self._dispatch(req)))
            except _ClientGone:
                pass
            except Exception as e:
                print(f"secret-broker: request failed: {e!r}", file=sys.stderr)
                with suppress(_ClientGone):
                    _sendall(conn, response_obj(Failed("internal error")))

    def _dispatch(self, req: Request) -> Response:
        match req:
            case ListReq():
                return Listed(tuple(self._broker.list_actions()))
            case RunReq():
                return self._broker.handle_run(req)
            case ExportReq():
                return self._export(req)

    def _export(self, req: ExportReq) -> Response:
        config = self._config
        if config.export_provider is None:
            return Denied("export disabled")
        try:
            spec_text, entries = spec.canonical_spec(
                req.spec, req.profiles, config.export_profiles, config.export_provider
            )
        except SpecError as e:
            return Denied(str(e))
        titles = {p: spec.resolved_titles(config.export_provider, p, es) for p, es in entries.items()}
        tag = new_tag()
        self._send(pending_obj(tag))
        notify("secret export request waits on the console")
        password = self.console.approve_export(req, tag, titles, self._ctx.cancel)
        if isinstance(password, Denied):
            return password
        try:
            with self.console.vault_session(password) as session:
                exported = run_export(self._secretspec, spec_text, list(entries), session, self._env)
        except (bitwarden.BwError, ExportError) as e:
            self.console.note(str(e))
            return Failed(str(e))
        return Exported(exported)


def main(config_path: Path) -> int:
    try:
        config = load_config_file(config_path)
    except ConfigError as e:
        print(f"secret-broker: {e}", file=sys.stderr)
        return 1
    missing = [n for n in ("SECRET_BROKER_BW", "SECRET_BROKER_SECRETSPEC", "STATE_DIRECTORY") if n not in os.environ]
    if missing:
        print(f"secret-broker: {', '.join(missing)} not set", file=sys.stderr)
        return 1
    try:
        uid = pwd.getpwnam(config.user).pw_uid
    except KeyError:
        print(f"secret-broker: unknown user {config.user}", file=sys.stderr)
        return 1
    bw = os.environ["SECRET_BROKER_BW"]
    secretspec = os.environ["SECRET_BROKER_SECRETSPEC"]
    state = Path(os.environ["STATE_DIRECTORY"])
    (state / "bw").mkdir(mode=0o700, exist_ok=True)
    env = {
        "PATH": f"{os.path.dirname(bw)}:{os.path.dirname(secretspec)}:{runner.FIXED_PATH}",
        "BITWARDENCLI_APPDATA_DIR": str(state / "bw"),
        "LANG": "C.UTF-8",
        "SSL_CERT_FILE": runner.CERTS,
        "NIX_SSL_CERT_FILE": runner.CERTS,
    }
    sock = listen_socket()
    disable_signal_keys(0)  # stdin is tty9 (StandardInput=tty)
    phrase = load_phrase(state, sys.stdin, sys.stdout)
    server = Server(config, uid, bw, secretspec, env, phrase, sys.stdin, sys.stdout)
    threading.Thread(target=server.console.loop, daemon=True).start()
    server.serve_forever(sock)
    return 0
