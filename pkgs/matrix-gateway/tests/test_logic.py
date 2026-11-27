import base64
import json

import pytest
from cryptography.exceptions import InvalidSignature
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from matrix_gateway.data import ParseError, loads
from matrix_gateway.logic import (
    POST_ROOMS,
    Send,
    content,
    parse_command,
    parse_send,
    sanitize,
    seal_command,
)


@pytest.mark.parametrize(
    ("text", "expected"),
    [
        ("a\x1bb", "a�b"),
        ("a‮b", "a�b"),  # noqa: PLE2502
        ("a\u200bb", "a�b"),
        ("a\x7fb", "a�b"),
        ("a\rb", "a�b"),
        ("a b", "a�b"),
        ("a b", "a�b"),
        ("a\nb", "a\nb"),
        ("a\tb", "a\tb"),
        ("plain ünïcode", "plain ünïcode"),
    ],
)
def test_sanitize(text: str, expected: str) -> None:
    assert sanitize(text) == expected
    assert sanitize(sanitize(text)) == sanitize(text)


def j(**kw: object) -> str:
    return json.dumps(kw)


@pytest.mark.parametrize(
    ("line", "expected"),
    [
        ('{"reply_to":"$e1","body":"ok","to":"yenn"}', Send("ok", reply_to="$e1")),
        ('{"reply_to":"$e1","body":"ok"}', Send("ok", reply_to="$e1")),
        (j(room="mail", body="x" * 20000), Send("x" * 20000, room="mail")),
        (j(reply_to="$" + "e" * 254, body="x"), Send("x", reply_to="$" + "e" * 254)),
    ],
    ids=[
        "notice-ignores-route",
        "notice",
        "body-20000",
        "reply-to-255",
    ],
)
def test_parse_send_valid(line: str, expected: Send) -> None:
    assert parse_send(line) == expected


INVALID = [
    "nil body",
    "",
    "[1]",
    '"str"',
    "1",
    "null",
    '{"body":"hi"}',
    '{"body":""}',
    '{"body":1}',
    '{"room":"mail","body":null}',
    j(room="mail", body="x" * 20001),
    '{"to":"yenn","ref":"r","body":"x"}',
    j(reply_to="e1", body="x"),
    j(reply_to=1, body="x"),
    j(reply_to="$" + "e" * 255, body="x"),
    j(reply_to="e1", to="yenn", ref="r", body="x"),
]


@pytest.mark.parametrize(
    "line", INVALID, ids=[f"invalid{i}" for i in range(len(INVALID))]
)
def test_parse_send_invalid(line: str) -> None:
    assert parse_send(line) is None


def test_content_notice() -> None:
    assert content(Send("ok", reply_to="$e1")) == {
        "msgtype": "m.notice",
        "body": "ok",
        "m.relates_to": {"m.in_reply_to": {"event_id": "$e1"}},
    }


def test_content_sanitizes() -> None:
    assert content(Send("a\x1bb", room="mail"))["body"] == "a�b"
    assert content(Send("a\x1bb", reply_to="$e"))["body"] == "a�b"


@pytest.mark.parametrize(
    "text", ["[" * 100000, '{"a":' * 100000], ids=["array", "object"]
)
def test_loads_deep_nesting_is_parse_error(text: str) -> None:
    with pytest.raises(ParseError):
        loads(text)


def test_loads_invalid_is_parse_error() -> None:
    with pytest.raises(ParseError):
        loads("nil")


LONE_SURROGATE = [
    r'{"room":"mail","body":"\ud800"}',
    r'{"room":"\ud800","body":"x"}',
    r'{"reply_to":"$\ud800","body":"x"}',
    r'{"reply_to":"$e","body":"\ud800"}',
    r'{"room":"mail","body":"a\udc00b"}',
]


@pytest.mark.parametrize(
    "line", LONE_SURROGATE, ids=[f"surrogate{i}" for i in range(len(LONE_SURROGATE))]
)
def test_parse_send_lone_surrogate(line: str) -> None:
    assert parse_send(line) is None


@pytest.mark.parametrize(
    "line",
    [
        '{"room":"mail","body":"hi"}',
        '{"room":"mail","body":"hi","to":"yenn","ref":"r"}',
    ],
)
def test_parse_send_post_room(line: str) -> None:
    assert parse_send(line) == Send("hi", room="mail")


@pytest.mark.parametrize(
    "line",
    [
        '{"room":"agents","body":"hi"}',
        '{"room":"x","body":"hi"}',
        '{"room":1,"body":"hi"}',
        '{"room":"mail","body":""}',
    ],
)
def test_parse_send_post_room_invalid(line: str) -> None:
    assert parse_send(line) is None


def test_content_post_room() -> None:
    assert content(Send("a\x07b", room="mail")) == {"msgtype": "m.text", "body": "a�b"}


def test_post_rooms() -> None:
    assert POST_ROOMS == ("mail", "alerts")


def test_parse_send_alerts_room() -> None:
    assert parse_send('{"room":"alerts","body":"hi"}') == Send("hi", room="alerts")


def test_parse_send_alerts_room_invalid() -> None:
    assert parse_send('{"room":"alerts","body":""}') is None


def test_content_alerts_room() -> None:
    assert content(Send("a\x07b", room="alerts")) == {
        "msgtype": "m.text",
        "body": "a�b",
    }


def test_parse_send_deeply_nested() -> None:
    assert parse_send("[" * 100000) is None


@pytest.mark.parametrize(
    ("body", "expected"),
    [
        ("!yenn fix the test", ("yenn", "fix the test")),
        ("!Yenn  fix\nmore ", ("yenn", "fix\nmore")),
        ("  !geralt x", ("geralt", "x")),
        ("!yenn\nx", ("yenn", "x")),
        ("!a-b1 x", ("a-b1", "x")),
        ("!" + "a" * 63 + " x", ("a" * 63, "x")),
    ],
)
def test_parse_command(body: str, expected: tuple[str, str]) -> None:
    assert parse_command(body) == expected


@pytest.mark.parametrize(
    "body",
    [
        "!yenn",
        "!yenn   ",
        "yenn fix",
        "! yenn x",
        "!yenn: x",
        "!a.b x",
        "!-x y",
        "!" + "a" * 64 + " x",
        "",
        "!",
        "!!",
    ],
)
def test_parse_command_none(body: str) -> None:
    assert parse_command(body) is None


def test_seal_command_verifies() -> None:
    key = Ed25519PrivateKey.generate()
    out = seal_command(key, "yenn", "line1\nline2 é", "$ev", 1700000000000)
    assert "\n" not in out
    env = json.loads(out)
    assert set(env) == {"type", "msg", "sig"}
    assert env["type"] == "matrix.command"
    key.public_key().verify(
        base64.b64decode(env["sig"], validate=True), env["msg"].encode("utf-8")
    )
    assert json.loads(env["msg"]) == {
        "kind": "matrix.command",
        "to": "yenn",
        "text": "line1\nline2 é",
        "event_id": "$ev",
        "ts": 1700000000000,
    }


def test_seal_command_wrong_key_fails() -> None:
    env = json.loads(seal_command(Ed25519PrivateKey.generate(), "yenn", "t", "$e", 1))
    with pytest.raises(InvalidSignature):
        Ed25519PrivateKey.generate().public_key().verify(
            base64.b64decode(env["sig"]), env["msg"].encode()
        )
