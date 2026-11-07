import base64
from collections.abc import Iterator

import pytest
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey, Ed25519PublicKey

from fakes import OWNER, ROOM, FakeBus, FakeMatrix
from matrix_gateway import cli
from matrix_gateway.gateway import Fatal, Gateway, MatrixError

LINE = '{"to":"yenn","ref":"r","body":"hi"}'


def test_keygen(capsys: pytest.CaptureFixture[str]) -> None:
    cli.keygen()
    lines = capsys.readouterr().out.splitlines()
    assert len(lines) == 2
    assert lines[0].startswith("GATEWAY_SIGNING_KEY=")
    assert lines[1].startswith("MATRIX_GATEWAY_KEY=")
    private = base64.b64decode(lines[0].split("=", 1)[1], validate=True)
    public = base64.b64decode(lines[1].split("=", 1)[1], validate=True)
    assert len(private) == 32
    assert len(public) == 32
    key = Ed25519PrivateKey.from_private_bytes(private)
    Ed25519PublicKey.from_public_bytes(public).verify(key.sign(b"msg"), b"msg")


def test_keygen_is_random(capsys: pytest.CaptureFixture[str]) -> None:
    cli.keygen()
    first = capsys.readouterr().out
    cli.keygen()
    assert capsys.readouterr().out != first


def make_gateway(matrix: FakeMatrix) -> Gateway:
    matrix.aliases["#agents:nkmrk.com"] = ROOM
    gw = Gateway(matrix, FakeBus(), OWNER, "agents", "pw", Ed25519PrivateKey.generate())
    gw.start()
    return gw


@pytest.fixture
def sleeps(monkeypatch: pytest.MonkeyPatch) -> list[float]:
    calls: list[float] = []
    monkeypatch.setattr(cli.time, "sleep", calls.append)
    return calls


@pytest.fixture
def exits(monkeypatch: pytest.MonkeyPatch) -> list[int]:
    codes: list[int] = []

    def fake_exit(code: int) -> None:
        codes.append(code)
        raise SystemExit(code)

    monkeypatch.setattr(cli.os, "_exit", fake_exit)
    return codes


def test_deliver_success(sleeps: list[float]) -> None:
    m = FakeMatrix()
    cli._deliver(make_gateway(m), LINE)
    assert len(m.sent) == 1
    assert m.sent[0].txn.startswith("send-")
    assert sleeps == []


def test_deliver_retries_with_same_txn_then_succeeds(sleeps: list[float]) -> None:
    m = FakeMatrix()
    gw = make_gateway(m)
    m.send_failures = [MatrixError("503")]
    cli._deliver(gw, LINE)
    assert len(m.send_attempts) == 2
    assert m.send_attempts[0] == m.send_attempts[1]
    assert len(m.sent) == 1
    assert m.sent[0].txn == m.send_attempts[0]
    assert sleeps == [5]


def test_deliver_gives_up_after_three_attempts(sleeps: list[float]) -> None:
    m = FakeMatrix()
    gw = make_gateway(m)
    m.send_failures = [MatrixError("1"), MatrixError("2"), MatrixError("3"), MatrixError("4")]
    cli._deliver(gw, LINE)
    assert len(m.send_attempts) == 3
    assert len(set(m.send_attempts)) == 1
    assert m.sent == []
    assert len(m.send_failures) == 1


def test_deliver_new_txn_per_line(sleeps: list[float]) -> None:
    m = FakeMatrix()
    gw = make_gateway(m)
    cli._deliver(gw, LINE)
    cli._deliver(gw, LINE)
    assert m.sent[0].txn != m.sent[1].txn


def test_deliver_invalid_line_sends_nothing(sleeps: list[float]) -> None:
    m = FakeMatrix()
    cli._deliver(make_gateway(m), "nil")
    assert m.sent == []


def test_pump_delivers_each_line_then_exits_1_when_sub_ends(sleeps: list[float], exits: list[int]) -> None:
    m = FakeMatrix()
    gw = make_gateway(m)
    with pytest.raises(SystemExit):
        cli._pump(gw, [LINE, LINE])  # type: ignore[arg-type]
    assert len(m.sent) == 2
    assert exits == [1]


def test_pump_exits_1_when_delivery_raises(sleeps: list[float], exits: list[int], monkeypatch: pytest.MonkeyPatch) -> None:
    gw = make_gateway(FakeMatrix())

    def boom(gateway: Gateway, line: str) -> None:
        raise RuntimeError("boom")

    monkeypatch.setattr(cli, "_deliver", boom)
    with pytest.raises(SystemExit):
        cli._pump(gw, [LINE])  # type: ignore[arg-type]
    assert exits == [1]


def test_pump_exits_1_on_fatal(sleeps: list[float], exits: list[int]) -> None:
    m = FakeMatrix()
    gw = make_gateway(m)
    m.send_failures = [Fatal("401")]
    with pytest.raises(SystemExit):
        cli._pump(gw, [LINE])  # type: ignore[arg-type]
    assert exits == [1]


def test_pump_exits_1_when_sub_raises(sleeps: list[float], exits: list[int]) -> None:
    gw = make_gateway(FakeMatrix())

    def lines() -> Iterator[str]:
        raise ValueError("decode")
        yield ""

    with pytest.raises(SystemExit):
        cli._pump(gw, lines())  # type: ignore[arg-type]
    assert exits == [1]
