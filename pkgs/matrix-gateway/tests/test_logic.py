import base64
import json
import re

import pytest
from cryptography.exceptions import InvalidSignature
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

from matrix_gateway.data import Obj, ParseError, loads
from matrix_gateway.logic import ROUTE, TO_RE, Send, content, parse_send, route, sanitize, seal, strip_fallback

BOT = "@agents:nkmrk.com"


@pytest.mark.parametrize(
    ("text", "expected"),
    [
        ("a\x1bb", "a�b"),
        ("a‮b", "a�b"),
        ("a​b", "a�b"),
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


def test_sanitize_single_line() -> None:
    assert sanitize("a\nb\tc\x1bd", single_line=True) == "a b c�d"


@pytest.mark.parametrize(
    ("body", "expected"),
    [
        ("> <@agents:nkmrk.com> q\n> more\n\nyes", "yes"),
        ("yes", "yes"),
        ("> only", ""),
        ("a\n> b", "a\n> b"),
    ],
)
def test_strip_fallback(body: str, expected: str) -> None:
    assert strip_fallback(body) == expected


def j(**kw: object) -> str:
    return json.dumps(kw)


@pytest.mark.parametrize(
    ("line", "expected"),
    [
        ('{"to":"yenn","ref":"/r/x/reply","body":"hi"}', Send("hi", "yenn", "/r/x/reply")),
        ('{"reply_to":"$e1","body":"ok","to":"yenn"}', Send("ok", reply_to="$e1")),
        ('{"reply_to":"$e1","body":"ok"}', Send("ok", reply_to="$e1")),
        (j(to="a", ref="r", body="x" * 20000), Send("x" * 20000, "a", "r")),
        (j(to="a" * 63, ref="r", body="x"), Send("x", "a" * 63, "r")),
        (j(to="0a-b", ref="r" * 1024, body="x"), Send("x", "0a-b", "r" * 1024)),
        (j(reply_to="$" + "e" * 254, body="x"), Send("x", reply_to="$" + "e" * 254)),
    ],
    ids=["routed", "notice-ignores-route", "notice", "body-20000", "to-63", "ref-1024", "reply-to-255"],
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
    '{"to":"yenn","ref":"r"}',
    '{"to":"yenn","ref":"r","body":null}',
    j(to="yenn", ref="r", body="x" * 20001),
    '{"to":"yenn","body":"x"}',
    '{"ref":"r","body":"x"}',
    j(to="Yenn", ref="r", body="x"),
    j(to="a.b", ref="r", body="x"),
    j(to="*", ref="r", body="x"),
    j(to=">", ref="r", body="x"),
    j(to="", ref="r", body="x"),
    j(to="-a", ref="r", body="x"),
    j(to="a" * 64, ref="r", body="x"),
    j(to=1, ref="r", body="x"),
    j(to="yenn", ref="", body="x"),
    j(to="yenn", ref=1, body="x"),
    j(to="yenn", ref="r" * 1025, body="x"),
    j(reply_to="e1", body="x"),
    j(reply_to=1, body="x"),
    j(reply_to="$" + "e" * 255, body="x"),
    j(reply_to="e1", to="yenn", ref="r", body="x"),
]


@pytest.mark.parametrize("line", INVALID, ids=[f"invalid{i}" for i in range(len(INVALID))])
def test_parse_send_invalid(line: str) -> None:
    assert parse_send(line) is None


def test_to_re() -> None:
    assert re.fullmatch(TO_RE, "yenn-2")
    for bad in ("Yenn", "a.b", "*", ">", "", "-a", "a" * 64):
        assert not re.fullmatch(TO_RE, bad)
    assert ROUTE == "com.nkmrk.route"


def test_content_routed() -> None:
    assert content(Send("hi", "yenn", "/r/x")) == {
        "msgtype": "m.text",
        "body": "hi",
        ROUTE: {"to": "yenn", "ref": "/r/x"},
    }


def test_content_notice() -> None:
    assert content(Send("ok", reply_to="$e1")) == {
        "msgtype": "m.notice",
        "body": "ok",
        "m.relates_to": {"m.in_reply_to": {"event_id": "$e1"}},
    }


def test_content_sanitizes() -> None:
    assert content(Send("a\x1bb", "yenn", "r"))["body"] == "a�b"
    assert content(Send("a\x1bb", reply_to="$e"))["body"] == "a�b"


def test_seal_verifies() -> None:
    key = Ed25519PrivateKey.generate()
    out = seal(key, "yenn", "/r/x", "line1\nline2 é", "B", "$ev")
    assert "\n" not in out
    env = json.loads(out)
    assert set(env) == {"type", "msg", "sig"}
    assert env["type"] == "matrix.reply"
    key.public_key().verify(base64.b64decode(env["sig"], validate=True), env["msg"].encode("utf-8"))
    assert json.loads(env["msg"]) == {
        "to": "yenn",
        "ref": "/r/x",
        "text": "line1\nline2 é",
        "body": "B",
        "event_id": "$ev",
    }


def test_seal_msg_is_sorted_compact() -> None:
    key = Ed25519PrivateKey.generate()
    env = json.loads(seal(key, "yenn", "r", "t", "b", "$e"))
    assert env["msg"] == '{"body":"b","event_id":"$e","ref":"r","text":"t","to":"yenn"}'
    env = json.loads(seal(key, "yenn", "r", "é", "b", "$e"))
    assert "é" in env["msg"]


def test_seal_wrong_key_fails() -> None:
    env = json.loads(seal(Ed25519PrivateKey.generate(), "yenn", "r", "t", "b", "$e"))
    with pytest.raises(InvalidSignature):
        Ed25519PrivateKey.generate().public_key().verify(base64.b64decode(env["sig"]), env["msg"].encode())


def parent(sender: str = BOT, **route_value: object) -> Obj:
    return {"sender": sender, "content": {"body": "B", ROUTE: route_value}}


def test_route_ok() -> None:
    assert route(parent(to="yenn", ref="/r/x"), BOT) == ("yenn", "/r/x")


@pytest.mark.parametrize(
    "p",
    [
        parent(sender="@other:nkmrk.com", to="yenn", ref="r"),
        parent(to="a.b", ref="r"),
        parent(to="Yenn", ref="r"),
        parent(to="yenn", ref=""),
        parent(to="yenn"),
        parent(ref="r"),
        parent(to="yenn", ref=1),
        {"sender": BOT, "content": {"body": "B"}},
        {"sender": BOT, "content": {"body": "B", ROUTE: "yenn"}},
        {"sender": BOT, "content": {"body": "B", ROUTE: ["yenn", "r"]}},
        {"sender": BOT},
        {"content": {ROUTE: {"to": "yenn", "ref": "r"}}},
    ],
)
def test_route_none(p: Obj) -> None:
    assert route(p, BOT) is None


@pytest.mark.parametrize("text", ["[" * 100000, '{"a":' * 100000], ids=["array", "object"])
def test_loads_deep_nesting_is_parse_error(text: str) -> None:
    with pytest.raises(ParseError):
        loads(text)


def test_loads_invalid_is_parse_error() -> None:
    with pytest.raises(ParseError):
        loads("nil")


LONE_SURROGATE = [
    r'{"to":"yenn","ref":"r","body":"\ud800"}',
    r'{"to":"yenn","ref":"\ud800","body":"x"}',
    r'{"to":"\ud800","ref":"r","body":"x"}',
    r'{"reply_to":"$\ud800","body":"x"}',
    r'{"reply_to":"$e","body":"\ud800"}',
    r'{"to":"yenn","ref":"r","body":"a\udc00b"}',
]


@pytest.mark.parametrize("line", LONE_SURROGATE, ids=[f"surrogate{i}" for i in range(len(LONE_SURROGATE))])
def test_parse_send_lone_surrogate(line: str) -> None:
    assert parse_send(line) is None


def test_parse_send_deeply_nested() -> None:
    assert parse_send("[" * 100000) is None
