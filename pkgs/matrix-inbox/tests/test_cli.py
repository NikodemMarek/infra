import base64
import io
import os
from collections.abc import Iterator
from pathlib import Path

import pytest
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

from matrix_inbox import cli
from matrix_inbox.data import Json, dumps

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
