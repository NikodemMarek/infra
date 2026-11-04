import os
import socket
import subprocess
import sys
from pathlib import Path

import pytest

from secret_broker.serve import listen_socket, peer_uid

SRC = str(Path(__file__).resolve().parent.parent / "src")


def test_peer_uid_socketpair() -> None:
    a, b = socket.socketpair(socket.AF_UNIX)
    with a, b:
        assert peer_uid(a) == os.getuid()
        assert peer_uid(b) == os.getuid()


def test_peer_uid_real_unix_socket(tmp_path: Path) -> None:
    path = str(tmp_path / "s.sock")
    srv = socket.socket(socket.AF_UNIX)
    srv.bind(path)
    srv.listen(1)
    cli = socket.socket(socket.AF_UNIX)
    cli.connect(path)
    conn, _ = srv.accept()
    with srv, cli, conn:
        assert peer_uid(conn) == os.getuid()
        assert peer_uid(cli) == os.getuid()


def test_listen_socket_exits_1_without_activation(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("LISTEN_FDS", raising=False)
    monkeypatch.delenv("LISTEN_PID", raising=False)
    with pytest.raises(SystemExit) as e:
        listen_socket()
    assert e.value.code == 1


def test_listen_socket_exits_1_for_foreign_pid(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("LISTEN_FDS", "1")
    monkeypatch.setenv("LISTEN_PID", str(os.getpid() + 1))
    with pytest.raises(SystemExit) as e:
        listen_socket()
    assert e.value.code == 1


def test_listen_socket_uses_fd_3(tmp_path: Path) -> None:
    path = str(tmp_path / "act.sock")
    srv = socket.socket(socket.AF_UNIX)
    srv.bind(path)
    srv.listen(1)
    code = (
        "import os, sys\n"
        "fd = int(sys.argv[1])\n"
        "if fd != 3:\n"
        "    os.dup2(fd, 3)\n"
        "os.environ['LISTEN_FDS'] = '1'\n"
        "os.environ['LISTEN_PID'] = str(os.getpid())\n"
        "from secret_broker.serve import listen_socket\n"
        "s = listen_socket()\n"
        "print(s.getsockname())\n"
    )
    try:
        r = subprocess.run(
            [sys.executable, "-c", code, str(srv.fileno())],
            env={**os.environ, "PYTHONPATH": SRC},
            pass_fds=(srv.fileno(),),
            capture_output=True,
            text=True,
            timeout=30,
        )
    finally:
        srv.close()
    assert r.returncode == 0, r.stderr
    assert r.stdout.strip() == path
