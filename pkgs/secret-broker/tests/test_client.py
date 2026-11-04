import base64
import json
import os
import socket
import subprocess
import sys
import threading
import time
from pathlib import Path

import pytest

SRC = str(Path(__file__).resolve().parent.parent / "src")
PENDING_RUN = "secret: waiting for approval on the console (Alt+SysRq+R, Ctrl+Alt+F9)…"


class Server:
    """One-shot fake broker: records each request line, answers with canned raw chunks, closes."""

    def __init__(self, path: Path, replies: list[bytes]) -> None:
        self.path = path
        self.replies = replies
        self.requests: list[bytes] = []
        self.connections = 0
        self.sock = socket.socket(socket.AF_UNIX)
        self.sock.bind(str(path))
        self.sock.listen(4)
        self.thread = threading.Thread(target=self.loop, daemon=True)
        self.thread.start()

    def loop(self) -> None:
        while True:
            try:
                conn, _ = self.sock.accept()
            except OSError:
                return
            self.connections += 1
            with conn:
                buf = b""
                while not buf.endswith(b"\n"):
                    chunk = conn.recv(65536)
                    if not chunk:
                        break
                    buf += chunk
                self.requests.append(buf)
                try:
                    for r in self.replies:
                        conn.sendall(r)
                        time.sleep(0.01)
                except OSError:
                    pass

    def request(self) -> dict[str, object]:
        assert len(self.requests) == 1
        assert self.requests[0].endswith(b"\n")
        assert self.requests[0].count(b"\n") == 1
        return json.loads(self.requests[0])

    def close(self) -> None:
        self.sock.close()


def line(obj: dict[str, object]) -> bytes:
    return json.dumps(obj, ensure_ascii=False, separators=(",", ":")).encode() + b"\n"


def ran(exit: int = 0, stdout: bytes = b"", stderr: bytes = b"", truncated: bool = False) -> bytes:
    return line(
        {
            "status": "ok",
            "exit": exit,
            "stdout": base64.b64encode(stdout).decode(),
            "stderr": base64.b64encode(stderr).decode(),
            "truncated": truncated,
        }
    )


@pytest.fixture
def make_server(tmp_path: Path):
    servers: list[Server] = []

    def make(*replies: bytes) -> Server:
        s = Server(tmp_path / f"b{len(servers)}.sock", list(replies))
        servers.append(s)
        return s

    yield make
    for s in servers:
        s.close()


def client(
    args: list[str],
    sock: Path,
    cwd: Path,
    session: str | None = None,
) -> subprocess.CompletedProcess[bytes]:
    env = {k: v for k, v in os.environ.items() if k not in ("SHPOOL_SESSION_NAME", "SECRET_BROKER_SOCKET")}
    env["PYTHONPATH"] = SRC
    env["SECRET_BROKER_SOCKET"] = str(sock)
    if session is not None:
        env["SHPOOL_SESSION_NAME"] = session
    code = "import sys; from secret_broker.client import main; sys.exit(main())"
    return subprocess.run(
        [sys.executable, "-c", code, *args], env=env, cwd=cwd, capture_output=True, timeout=30
    )


# usage


@pytest.mark.parametrize(
    "args",
    [[], ["bogus"], ["run"], ["export"]],
)
def test_usage_errors_exit_64(tmp_path: Path, make_server, args: list[str]) -> None:
    srv = make_server(line({"status": "ok", "actions": []}))
    r = client(args, srv.path, tmp_path)
    assert r.returncode == 64
    assert srv.connections == 0


# connect failure


def test_connect_failed_exit_69(tmp_path: Path) -> None:
    missing = tmp_path / "nope.sock"
    r = client(["list"], missing, tmp_path)
    assert r.returncode == 69
    assert r.stderr.decode().strip() == f"secret: broker not reachable ({missing})"


def test_connect_failed_when_path_is_not_a_socket(tmp_path: Path) -> None:
    f = tmp_path / "plain"
    f.write_text("x")
    assert client(["list"], f, tmp_path).returncode == 69


# list


def test_list_output(tmp_path: Path, make_server) -> None:
    srv = make_server(
        line(
            {
                "status": "ok",
                "actions": [
                    {
                        "name": "deploy",
                        "description": "Deploy it",
                        "args": ["^[a-z]+$", "^[0-9]+$"],
                        "items": ["tok"],
                        "state": "granted",
                        "detail": "12 min left",
                    },
                    {
                        "name": "ping",
                        "description": "Ping",
                        "args": [],
                        "items": [],
                        "state": "ready",
                        "detail": "",
                    },
                ],
            }
        )
    )
    r = client(["list"], srv.path, tmp_path)
    assert r.returncode == 0
    assert srv.request()["op"] == "list"
    assert r.stdout.decode().splitlines() == [
        "deploy\tgranted 12 min left\tDeploy it\t^[a-z]+$ ^[0-9]+$",
        "ping\tready\tPing\t",
    ]


def test_list_empty(tmp_path: Path, make_server) -> None:
    srv = make_server(line({"status": "ok", "actions": []}))
    r = client(["list"], srv.path, tmp_path)
    assert r.returncode == 0
    assert r.stdout == b""


# run


def test_run_request_shape_and_exit_code(tmp_path: Path, make_server) -> None:
    srv = make_server(ran(exit=3, stdout=b"out", stderr=b"err"))
    r = client(["run", "deploy", "x", "--flag", "-h", "--", "a b"], srv.path, tmp_path, session="sess1")
    assert r.returncode == 3
    req = srv.request()
    assert req["op"] == "run"
    assert req["action"] == "deploy"
    assert req["args"] == ["x", "--flag", "-h", "--", "a b"]
    assert isinstance(req["cwd"], str)
    assert os.path.realpath(str(req["cwd"])) == os.path.realpath(tmp_path)
    assert req["session"] == "sess1"
    assert r.stdout == b"out"
    assert r.stderr == b"err"


def test_run_session_defaults_to_empty(tmp_path: Path, make_server) -> None:
    srv = make_server(ran())
    client(["run", "a"], srv.path, tmp_path)
    assert srv.request()["session"] == ""
    assert srv.request()["args"] == []


def test_run_binary_output_is_raw(tmp_path: Path, make_server) -> None:
    out = bytes(range(256)) * 3
    err = b"\xff\xfe\x00bad utf8\n"
    srv = make_server(ran(stdout=out, stderr=err))
    r = client(["run", "a"], srv.path, tmp_path)
    assert r.returncode == 0
    assert r.stdout == out
    assert r.stderr == err


def test_run_large_output_in_chunks(tmp_path: Path, make_server) -> None:
    out = os.urandom(300_000)
    full = ran(stdout=out)
    srv = make_server(full[:1000], full[1000:100_000], full[100_000:])
    r = client(["run", "a"], srv.path, tmp_path)
    assert r.returncode == 0
    assert r.stdout == out


def test_run_truncated_message(tmp_path: Path, make_server) -> None:
    srv = make_server(ran(stdout=b"x", truncated=True))
    r = client(["run", "a"], srv.path, tmp_path)
    assert r.returncode == 0
    assert b"secret: output truncated" in r.stderr


def test_run_not_truncated_has_no_message(tmp_path: Path, make_server) -> None:
    srv = make_server(ran(stdout=b"x"))
    r = client(["run", "a"], srv.path, tmp_path)
    assert b"truncated" not in r.stderr


def test_run_exit_zero_and_signal_codes_pass_through(tmp_path: Path, make_server) -> None:
    for code in (0, 1, 143):
        srv = make_server(ran(exit=code))
        assert client(["run", "a"], srv.path, tmp_path).returncode == code


def test_run_pending_message_without_tag(tmp_path: Path, make_server) -> None:
    srv = make_server(line({"status": "pending"}), ran(stdout=b"done"))
    r = client(["run", "a"], srv.path, tmp_path)
    assert r.returncode == 0
    assert r.stdout == b"done"
    assert PENDING_RUN in r.stderr.decode()
    assert "tag" not in r.stderr.decode()


def test_run_denied(tmp_path: Path, make_server) -> None:
    srv = make_server(line({"status": "denied", "reason": "denied on the console"}))
    r = client(["run", "a"], srv.path, tmp_path)
    assert r.returncode == 77
    assert r.stderr.decode().strip() == "secret: denied: denied on the console"
    assert r.stdout == b""


def test_run_denied_after_pending(tmp_path: Path, make_server) -> None:
    srv = make_server(line({"status": "pending"}), line({"status": "denied", "reason": "approval timed out"}))
    r = client(["run", "a"], srv.path, tmp_path)
    assert r.returncode == 77
    assert "secret: denied: approval timed out" in r.stderr.decode()
    assert PENDING_RUN in r.stderr.decode()


def test_run_error_status(tmp_path: Path, make_server) -> None:
    srv = make_server(line({"status": "error", "reason": "action timed out after 5 s"}))
    r = client(["run", "a"], srv.path, tmp_path)
    assert r.returncode == 70
    assert r.stderr.decode().strip() == "secret: error: action timed out after 5 s"


def test_eof_before_final_line(tmp_path: Path, make_server) -> None:
    srv = make_server()
    r = client(["run", "a"], srv.path, tmp_path)
    assert r.returncode == 70
    assert r.stderr.decode().strip() == "secret: broker closed the connection"


def test_eof_after_pending(tmp_path: Path, make_server) -> None:
    srv = make_server(line({"status": "pending"}))
    r = client(["run", "a"], srv.path, tmp_path)
    assert r.returncode == 70
    assert "secret: broker closed the connection" in r.stderr.decode()


def test_eof_in_the_middle_of_a_line(tmp_path: Path, make_server) -> None:
    srv = make_server(b'{"status":"ok","exit":0,')
    r = client(["run", "a"], srv.path, tmp_path)
    assert r.returncode == 70


def test_list_error_status(tmp_path: Path, make_server) -> None:
    srv = make_server(line({"status": "error", "reason": "bad request"}))
    r = client(["list"], srv.path, tmp_path)
    assert r.returncode == 70
    assert r.stderr.decode().strip() == "secret: error: bad request"


# export


def test_export_missing_secretspec_toml_exits_64_without_connecting(tmp_path: Path, make_server) -> None:
    srv = make_server(line({"status": "ok", "profiles": {}}))
    r = client(["export", "a"], srv.path, tmp_path)
    assert r.returncode == 64
    assert srv.connections == 0


def test_export_missing_secretspec_toml_with_no_broker_is_still_64(tmp_path: Path) -> None:
    assert client(["export", "a"], tmp_path / "nope.sock", tmp_path).returncode == 64


def test_export_request_and_output(tmp_path: Path, make_server) -> None:
    spec = '[project]\nname = "ü"\n\n[profiles.a]\nK = {}\n'
    (tmp_path / "secretspec.toml").write_text(spec, encoding="utf-8")
    profiles = {"a": {"K": "välue"}, "b": {"X": "1", "Y": "2"}}
    srv = make_server(line({"status": "ok", "profiles": profiles}))
    r = client(["export", "a", "b"], srv.path, tmp_path, session="s9")
    assert r.returncode == 0
    req = srv.request()
    assert req["op"] == "export"
    assert req["profiles"] == ["a", "b"]
    assert req["spec"] == spec
    assert os.path.realpath(str(req["cwd"])) == os.path.realpath(tmp_path)
    assert req["session"] == "s9"
    assert r.stdout == json.dumps(profiles, ensure_ascii=False, separators=(",", ":")).encode() + b"\n"


def test_export_pending_with_tag(tmp_path: Path, make_server) -> None:
    (tmp_path / "secretspec.toml").write_text("[project]\n")
    srv = make_server(
        line({"status": "pending", "tag": "abcd"}),
        line({"status": "ok", "profiles": {"a": {"K": "v"}}}),
    )
    r = client(["export", "a"], srv.path, tmp_path)
    assert r.returncode == 0
    assert (
        "secret: approve on the console only if it shows tag abcd (Alt+SysRq+R, Ctrl+Alt+F9)" in r.stderr.decode()
    )
    assert json.loads(r.stdout) == {"a": {"K": "v"}}


def test_export_denied(tmp_path: Path, make_server) -> None:
    (tmp_path / "secretspec.toml").write_text("[project]\n")
    srv = make_server(line({"status": "denied", "reason": "export disabled"}))
    r = client(["export", "a"], srv.path, tmp_path)
    assert r.returncode == 77
    assert r.stderr.decode().strip() == "secret: denied: export disabled"
    assert r.stdout == b""


def test_export_error(tmp_path: Path, make_server) -> None:
    (tmp_path / "secretspec.toml").write_text("[project]\n")
    srv = make_server(line({"status": "error", "reason": "wrong master password"}))
    r = client(["export", "a"], srv.path, tmp_path)
    assert r.returncode == 70
    assert r.stderr.decode().strip() == "secret: error: wrong master password"


def test_export_eof(tmp_path: Path, make_server) -> None:
    (tmp_path / "secretspec.toml").write_text("[project]\n")
    srv = make_server(line({"status": "pending", "tag": "wxyz"}))
    r = client(["export", "a"], srv.path, tmp_path)
    assert r.returncode == 70
    assert "secret: broker closed the connection" in r.stderr.decode()
