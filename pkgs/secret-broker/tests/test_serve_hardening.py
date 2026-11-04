"""S2 hardening of serve: peer check, connection cap, deadlines, accept-loop survival, error mapping."""

import io
import json
import os
import socket
import sys
import threading
import time
from collections.abc import Callable, Iterator
from pathlib import Path

import pytest

from secret_broker import console as console_mod
from secret_broker import runner, serve
from secret_broker.model import Action, Config, Listed
from secret_broker.protocol import encode, response_obj
from secret_broker.serve import Server

REQ = b'{"op":"list"}\n'


def make_config(n_actions: int = 1, desc: str = "d") -> Config:
    actions = {
        f"a{i:05d}": Action(f"a{i:05d}", desc, ("/nix/store/x/bin/true",), (), {}, 5) for i in range(n_actions)
    }
    return Config("u", 240, None, frozenset(), None, actions)


def make_server(config: Config | None = None, uid: int | None = None) -> Server:
    return Server(
        config or make_config(),
        os.getuid() if uid is None else uid,
        "/nonexistent/bw",
        "/nonexistent/secretspec",
        {},
        "phrase",
        io.StringIO(),
        io.StringIO(),
    )


@pytest.fixture
def listener(tmp_path: Path) -> Iterator[Callable[[Server], str]]:
    socks: list[socket.socket] = []

    def start(server: Server) -> str:
        path = str(tmp_path / "s.sock")
        s = socket.socket(socket.AF_UNIX)
        s.bind(path)
        s.listen(64)
        socks.append(s)
        threading.Thread(target=server.serve_forever, args=(s,), daemon=True).start()
        return path

    yield start
    for s in socks:
        s.close()


def connect(path: str) -> socket.socket:
    c = socket.socket(socket.AF_UNIX)
    c.settimeout(10)
    c.connect(path)
    return c


def read_all(c: socket.socket) -> bytes:
    """Everything until EOF; ConnectionResetError counts as EOF."""
    buf = b""
    try:
        while chunk := c.recv(65536):
            buf += chunk
    except ConnectionResetError:
        pass
    return buf


def request(path: str, line: bytes = REQ) -> dict[str, object]:
    with connect(path) as c:
        c.sendall(line)
        return json.loads(read_all(c).splitlines()[0])


def is_open(c: socket.socket) -> bool:
    """True when the server has not closed `c` (recv times out instead of EOF)."""
    c.settimeout(0.3)
    try:
        return c.recv(1) != b""
    except TimeoutError:
        return True
    except OSError:
        return False


def test_foreign_uid_gets_eof_without_reply() -> None:
    server = make_server(uid=os.getuid() + 1)
    a, b = socket.socketpair(socket.AF_UNIX)
    b.settimeout(10)
    with a, b:
        b.sendall(REQ)
        t = threading.Thread(target=server._handle, args=(a,), daemon=True)
        t.start()
        assert read_all(b) == b""
        t.join(5)
        assert not t.is_alive()


def test_own_uid_is_served() -> None:
    server = make_server()
    a, b = socket.socketpair(socket.AF_UNIX)
    b.settimeout(10)
    with a, b:
        b.sendall(REQ)
        threading.Thread(target=server._handle, args=(a,), daemon=True).start()
        assert json.loads(read_all(b).splitlines()[0])["status"] == "ok"


def test_ninth_concurrent_connection_is_closed_first_eight_stay(listener: Callable[[Server], str]) -> None:
    path = listener(make_server())
    first = [connect(path) for _ in range(8)]
    time.sleep(0.5)
    extra = connect(path)
    try:
        assert read_all(extra) == b""  # EOF without a reply, promptly (socket timeout is 10 s)
        assert all(is_open(c) for c in first)
    finally:
        for c in [*first, extra]:
            c.close()


def test_slot_is_freed_when_a_connection_closes(listener: Callable[[Server], str]) -> None:
    path = listener(make_server())
    first = [connect(path) for _ in range(8)]
    time.sleep(0.3)
    first.pop().close()
    time.sleep(0.5)
    assert request(path)["status"] == "ok"
    for c in first:
        c.close()


def test_trickling_client_is_dropped_after_total_deadline(
    monkeypatch: pytest.MonkeyPatch, listener: Callable[[Server], str]
) -> None:
    monkeypatch.setattr(serve, "REQUEST_TIMEOUT", 1)  # the total request deadline
    path = listener(make_server())
    start = time.monotonic()
    dropped = False
    with connect(path) as c:
        while time.monotonic() - start < 6:
            try:
                c.sendall(b" ")  # never a newline
                c.settimeout(0.5)  # doubles as the trickle pace
                if c.recv(1) == b"":
                    dropped = True
                    break
            except TimeoutError:
                pass
            except OSError:
                dropped = True
                break
    assert dropped, "trickling client was never dropped"
    assert time.monotonic() - start < 4


def test_client_that_never_reads_does_not_wedge_the_server(
    monkeypatch: pytest.MonkeyPatch, listener: Callable[[Server], str]
) -> None:
    # the name of the sendall timeout constant is not in the contract; SEND_TIMEOUT is assumed
    monkeypatch.setattr(serve, "SEND_TIMEOUT", 1, raising=False)
    server = make_server(make_config(4000, "x" * 200))  # reply far larger than the socket buffers
    path = listener(server)
    stuck = connect(path)
    stuck.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 1024)
    stuck.sendall(REQ)  # and never read
    try:
        assert request(path)["status"] == "ok"  # others are still served
        time.sleep(3)  # > SEND_TIMEOUT: the stuck handler must have given up by now
        stuck.settimeout(10)
        total = 0
        try:
            while chunk := stuck.recv(1 << 20):
                total += len(chunk)
        except ConnectionResetError:
            pass
        full = len(encode(response_obj(Listed(tuple(server._broker.list_actions())))))
        assert total < full, "the whole reply was still delivered: sendall did not time out"
    finally:
        stuck.close()


def test_accept_loop_survives_thread_start_error(
    monkeypatch: pytest.MonkeyPatch, listener: Callable[[Server], str]
) -> None:
    path = listener(make_server())
    real = threading.Thread.start
    calls: list[int] = []

    def flaky(self: threading.Thread) -> None:
        if not calls:
            calls.append(1)
            raise RuntimeError("can't start new thread")
        real(self)

    monkeypatch.setattr(threading.Thread, "start", flaky)
    with connect(path) as victim:
        victim.sendall(REQ)
        read_all(victim)  # whatever happens to this one, it must end
    assert calls == [1]
    assert request(path)["status"] == "ok"
    assert request(path)["status"] == "ok"


def test_internal_error_reply_and_server_keeps_running(
    monkeypatch: pytest.MonkeyPatch, listener: Callable[[Server], str]
) -> None:
    server = make_server()
    server._broker.unlock({}, {})
    monkeypatch.setattr(server.console, "ask", lambda pending, cancel: "once")
    monkeypatch.setattr(serve, "notify", lambda text: None)

    def boom(*a: object, **k: object) -> object:
        raise FileNotFoundError("/nix/store/x/bin/true")

    monkeypatch.setattr(runner, "run_action", boom)
    path = listener(server)
    with connect(path) as c:
        c.sendall(b'{"op":"run","action":"a00000","args":[]}\n')
        lines = [json.loads(ln) for ln in read_all(c).splitlines()]
    assert lines[-1] == {"status": "error", "reason": "internal error"}
    assert request(path)["status"] == "ok"


FAKE_BW = """#!{py}
import json, os, sys
log = os.environ["FAKE_LOG"]
state = log + ".server"
with open(log, "a") as f:
    f.write(json.dumps(sys.argv[1:]) + "\\n")
url = open(state).read() if os.path.exists(state) else None
if sys.argv[1:2] == ["status"]:
    print(json.dumps({{"status": "unauthenticated", "serverUrl": url}}))
elif sys.argv[1:3] == ["config", "server"]:
    open(state, "w").write(sys.argv[3])
sys.exit(0)
"""


def test_first_login_sets_bw_server_before_login(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    # never sweep the real cgroup of the test run
    monkeypatch.setattr(console_mod, "sweep", lambda *a, **k: True)
    bw = tmp_path / "bw"
    bw.write_text(FAKE_BW.format(py=sys.executable))
    bw.chmod(0o755)
    log = tmp_path / "log"
    url = "https://vault.example.org"
    cfg = Config("u", 240, None, frozenset(), url, {})
    with open(os.devnull) as tty_in, open(os.devnull, "w") as tty_out:
        server = Server(cfg, os.getuid(), str(bw), "/x", {"FAKE_LOG": str(log)}, "p", tty_in, tty_out)
        problem = server.console._ready()
    argvs = [json.loads(ln) for ln in log.read_text().splitlines()]
    assert problem is None
    assert ["config", "server", url] in argvs
    assert ["login", "--apikey"] in argvs
    assert argvs.index(["config", "server", url]) < argvs.index(["login", "--apikey"])
