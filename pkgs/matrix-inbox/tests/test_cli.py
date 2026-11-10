import base64
import io
import os
import time
from collections.abc import Callable, Iterator
from pathlib import Path

import pytest
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

from fakes import FakeBus, FakeSeen, FakeTasks
from matrix_inbox import cli
from matrix_inbox.data import Json, dumps
from matrix_inbox.handlers import BusError, Seen, Tasks
from matrix_inbox.signed import Command

FIELDS: dict[str, Json] = {"to": "yenn", "ref": "/r/x/reply", "text": "y", "body": "B", "event_id": "$e1"}


def public_b64(key: Ed25519PrivateKey) -> str:
    raw = key.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw)
    return base64.b64encode(raw).decode()


def signed_line(key: Ed25519PrivateKey) -> str:
    msg = dumps(FIELDS, sort_keys=True)
    sig = base64.b64encode(key.sign(msg.encode("utf-8"))).decode()
    return dumps({"type": "matrix.reply", "msg": msg, "sig": sig})


class Forbidden:
    def __init__(self, *args: object, **kwargs: object) -> None:
        raise AssertionError("must not be reached")


@pytest.fixture
def guarded(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(cli, "ProcessLocal", Forbidden)
    monkeypatch.setattr(cli, "NatsBus", Forbidden)
    monkeypatch.setattr(cli, "answer_reply", Forbidden)
    monkeypatch.setattr("sys.argv", ["matrix-inbox-answer"])


@pytest.fixture
def stdin(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> Iterator[None]:
    """Hands `set_stdin` a way to feed both fd 0 and sys.stdin; restores fd 0 afterwards."""
    saved = os.dup(0)
    try:
        yield None
    finally:
        os.dup2(saved, 0)
        os.close(saved)


@pytest.fixture
def feed(stdin: None, monkeypatch: pytest.MonkeyPatch, tmp_path: Path):  # type: ignore[no-untyped-def]
    def set_stdin(data: str) -> None:
        path = tmp_path / "stdin"
        path.write_bytes(data.encode("utf-8"))
        fd = os.open(path, os.O_RDONLY)
        os.dup2(fd, 0)
        os.close(fd)
        monkeypatch.setattr("sys.stdin", io.TextIOWrapper(open(0, "rb", closefd=False), encoding="utf-8"))

    return set_stdin


def run_answer() -> int:
    with pytest.raises(SystemExit) as exc:
        cli.answer()
    code = exc.value.code
    assert isinstance(code, int)
    return code


def test_key_unset_exits_2(guarded: None, feed, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]) -> None:  # type: ignore[no-untyped-def]
    monkeypatch.delenv("MATRIX_GATEWAY_KEY", raising=False)
    feed(signed_line(Ed25519PrivateKey.generate()))
    assert run_answer() == 2
    assert "set MATRIX_GATEWAY_KEY to the gateway's public key" in capsys.readouterr().err


@pytest.mark.parametrize(
    "value",
    ["", "!!!not base64", "AAAA", base64.b64encode(b"x" * 31).decode(), base64.b64encode(b"x" * 33).decode()],
    ids=["empty", "not-base64", "short", "31-bytes", "33-bytes"],
)
def test_key_invalid_exits_2(value: str, guarded: None, feed, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]) -> None:  # type: ignore[no-untyped-def]
    monkeypatch.setenv("MATRIX_GATEWAY_KEY", value)
    feed(signed_line(Ed25519PrivateKey.generate()))
    assert run_answer() == 2
    assert "set MATRIX_GATEWAY_KEY to the gateway's public key" in capsys.readouterr().err


@pytest.mark.parametrize("kind", ["unsigned", "plain", "empty", "wrong-key", "tampered"])
def test_bad_line_is_dropped_with_exit_1(kind: str, guarded: None, feed, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]) -> None:  # type: ignore[no-untyped-def]
    key = Ed25519PrivateKey.generate()
    monkeypatch.setenv("MATRIX_GATEWAY_KEY", public_b64(key))
    lines = {
        "unsigned": dumps({"type": "matrix.reply", "msg": dumps(FIELDS, sort_keys=True)}),
        "plain": "nil body",
        "empty": "",
        "wrong-key": signed_line(Ed25519PrivateKey.generate()),
        "tampered": signed_line(key).replace('\\"text\\":\\"y\\"', '\\"text\\":\\"z\\"'),
    }
    feed(lines[kind] + "\n")
    assert run_answer() == 1
    assert "dropping an unsigned or invalid reply" in capsys.readouterr().err


def test_args_exit_2(guarded: None, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr("sys.argv", ["matrix-inbox-answer", "extra"])
    assert run_answer() == 2


COMMAND_ERR = "matrix-inbox-command: "
KEY_HINT = "set MATRIX_GATEWAY_KEY to the gateway's public key"


def command_line(key: Ed25519PrivateKey, text: str = "fix it") -> str:
    fields: dict[str, Json] = {
        "kind": "matrix.command",
        "to": "yenn",
        "text": text,
        "event_id": "$c1",
        "ts": int(time.time() * 1000),
    }
    msg = dumps(fields, sort_keys=True)
    sig = base64.b64encode(key.sign(msg.encode("utf-8"))).decode()
    return dumps({"type": "matrix.command", "msg": msg, "sig": sig})


def run_command_cli() -> int:
    with pytest.raises(SystemExit) as exc:
        cli.command()
    code = exc.value.code
    assert isinstance(code, int)
    return code


@pytest.fixture
def guarded_command(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(cli, "SeenDir", Forbidden)
    monkeypatch.setattr(cli, "ProcessTasks", Forbidden)
    monkeypatch.setattr(cli, "NatsBus", Forbidden)
    monkeypatch.setattr(cli, "run_command", Forbidden)
    monkeypatch.setattr("sys.argv", ["matrix-inbox-command"])


def test_command_key_unset_exits_2(guarded_command: None, feed, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]) -> None:  # type: ignore[no-untyped-def]
    monkeypatch.delenv("MATRIX_GATEWAY_KEY", raising=False)
    feed(command_line(Ed25519PrivateKey.generate()))
    assert run_command_cli() == 2
    assert COMMAND_ERR + KEY_HINT in capsys.readouterr().err


@pytest.mark.parametrize(
    "value",
    ["", "!!!not base64", "AAAA", base64.b64encode(b"x" * 31).decode(), base64.b64encode(b"x" * 33).decode()],
    ids=["empty", "not-base64", "short", "31-bytes", "33-bytes"],
)
def test_command_key_invalid_exits_2(value: str, guarded_command: None, feed, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]) -> None:  # type: ignore[no-untyped-def]
    monkeypatch.setenv("MATRIX_GATEWAY_KEY", value)
    feed(command_line(Ed25519PrivateKey.generate()))
    assert run_command_cli() == 2
    assert KEY_HINT in capsys.readouterr().err


def test_command_args_exit_2(guarded_command: None, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr("sys.argv", ["matrix-inbox-command", "extra"])
    assert run_command_cli() == 2


@pytest.mark.parametrize("kind", ["unsigned", "plain", "empty", "wrong-key", "tampered", "reply-envelope"])
def test_command_bad_line_is_dropped_with_exit_1(kind: str, guarded_command: None, feed, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]) -> None:  # type: ignore[no-untyped-def]
    key = Ed25519PrivateKey.generate()
    monkeypatch.setenv("MATRIX_GATEWAY_KEY", public_b64(key))
    valid = command_line(key)
    tampered = valid.replace('\\"text\\":\\"fix it\\"', '\\"text\\":\\"rm -rf\\"')
    assert tampered != valid
    lines = {
        "unsigned": dumps({"type": "matrix.command", "msg": dumps({"kind": "matrix.command"}, sort_keys=True)}),
        "plain": "nil body",
        "empty": "",
        "wrong-key": command_line(Ed25519PrivateKey.generate()),
        "tampered": tampered,
        "reply-envelope": signed_line(key),
    }
    feed(lines[kind] + "\n")
    assert run_command_cli() == 1
    assert "dropping an unsigned or invalid command" in capsys.readouterr().err


class Wired:
    def __init__(self, seen: FakeSeen, tasks: FakeTasks, bus: FakeBus) -> None:
        self.seen = seen
        self.tasks = tasks
        self.bus = bus
        self.calls: list[tuple[Command, str, float, Seen, Tasks, FakeBus]] = []
        self.error: Exception | None = None

    def run(self, cmd: Command, host: str, now: float, seen: Seen, tasks: Tasks, bus: FakeBus) -> None:
        self.calls.append((cmd, host, now, seen, tasks, bus))
        if self.error is not None:
            raise self.error


@pytest.fixture
def wired(feed, monkeypatch: pytest.MonkeyPatch) -> Callable[..., Wired]:  # type: ignore[no-untyped-def]
    def setup(seen_error: OSError | None = None, real_handler: bool = False) -> Wired:
        key = Ed25519PrivateKey.generate()
        monkeypatch.setenv("MATRIX_GATEWAY_KEY", public_b64(key))
        monkeypatch.setattr("sys.argv", ["matrix-inbox-command"])
        monkeypatch.setattr(cli, "_host", lambda: "yenn")
        w = Wired(FakeSeen(error=seen_error), FakeTasks({"type": "task.started"}), FakeBus())
        monkeypatch.setattr(cli, "SeenDir", lambda path: w.seen)
        monkeypatch.setattr(cli, "ProcessTasks", lambda: w.tasks)
        monkeypatch.setattr(cli, "NatsBus", lambda url: w.bus)
        if not real_handler:
            monkeypatch.setattr(cli, "run_command", w.run)
        feed(command_line(key) + "\n")
        return w

    return setup


def test_command_valid_runs_with_host_and_command(wired: Callable[..., Wired]) -> None:
    w = wired()
    before = time.time()
    cli.command()
    assert len(w.calls) == 1
    cmd, host, now, seen, tasks, bus = w.calls[0]
    assert cmd.to == "yenn" and cmd.text == "fix it" and cmd.event_id == "$c1"
    assert host == "yenn"
    assert before <= now <= time.time()
    assert seen is w.seen and tasks is w.tasks and bus is w.bus


def test_command_bus_error_exits_1_with_one_line(wired: Callable[..., Wired], capsys: pytest.CaptureFixture[str]) -> None:
    w = wired()
    w.error = BusError("message bus unavailable")
    assert run_command_cli() == 1
    assert capsys.readouterr().err == COMMAND_ERR + "message bus unavailable\n"


@pytest.mark.parametrize("error", [PermissionError(13, "Permission denied"), FileNotFoundError(2, "No such file")])
def test_command_oserror_exits_1_with_one_line(error: OSError, wired: Callable[..., Wired], capsys: pytest.CaptureFixture[str]) -> None:
    w = wired()
    w.error = error
    assert run_command_cli() == 1
    err = capsys.readouterr().err
    assert err.startswith(COMMAND_ERR)
    assert err.count("\n") == 1 and err.endswith("\n")
    assert "Traceback" not in err


def test_command_seen_dir_oserror_exits_1_without_start(wired: Callable[..., Wired], capsys: pytest.CaptureFixture[str]) -> None:
    w = wired(seen_error=OSError(30, "Read-only file system"), real_handler=True)
    assert run_command_cli() == 1
    err = capsys.readouterr().err
    assert err.startswith(COMMAND_ERR)
    assert err.count("\n") == 1
    assert "Traceback" not in err
    assert w.tasks.prompts == []
    assert w.bus.published == []
