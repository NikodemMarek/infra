import pytest

from secret_broker import data
from secret_broker.data import Obj
from secret_broker.model import (
    ActionInfo,
    Denied,
    Exported,
    Failed,
    ListReq,
    Listed,
    Ran,
    RunReq,
    ExportReq,
)
from secret_broker.protocol import (
    MAX_LINE,
    ProtocolError,
    encode,
    parse_request,
    pending_obj,
    response_obj,
)


def test_max_line() -> None:
    assert MAX_LINE == 1 << 20


def test_list() -> None:
    assert parse_request(b'{"op":"list"}') == ListReq()
    assert parse_request(b'{"op":"list","extra":1}\n') == ListReq()


def test_run_minimal() -> None:
    assert parse_request(b'{"op":"run","action":"a","args":[]}') == RunReq("a", ())


def test_run_full() -> None:
    req = parse_request(
        b'{"op":"run","action":"a","args":["x","y"],"cwd":"/w","session":"s","junk":[1]}'
    )
    assert req == RunReq("a", ("x", "y"), "/w", "s")


def test_export_minimal() -> None:
    assert parse_request(b'{"op":"export","profiles":["p"],"spec":"t"}') == ExportReq(("p",), "t")


def test_export_full() -> None:
    req = parse_request(
        b'{"op":"export","profiles":["p","q"],"spec":"t","cwd":"/w","session":"s","z":0}'
    )
    assert req == ExportReq(("p", "q"), "t", "/w", "s")


@pytest.mark.parametrize(
    "raw",
    [
        b"",
        b"not json",
        b"{",
        b"[]",
        b'"list"',
        b"null",
        b"1",
        b"{}",
        b'{"op":"nope"}',
        b'{"op":1}',
        b'{"op":null}',
        b'{"op":"run"}',
        b'{"op":"run","action":"a"}',
        b'{"op":"run","args":[]}',
        b'{"op":"run","action":1,"args":[]}',
        b'{"op":"run","action":"a","args":"x"}',
        b'{"op":"run","action":"a","args":[1]}',
        b'{"op":"run","action":"a","args":[],"cwd":1}',
        b'{"op":"run","action":"a","args":[],"session":[]}',
        b'{"op":"export","spec":"t"}',
        b'{"op":"export","profiles":["p"]}',
        b'{"op":"export","profiles":[],"spec":"t"}',
        b'{"op":"export","profiles":"p","spec":"t"}',
        b'{"op":"export","profiles":[1],"spec":"t"}',
        b'{"op":"export","profiles":["p"],"spec":1}',
        b'{"op":"export","profiles":["p"],"spec":"t","cwd":1}',
        b'{"op":"list"} {"op":"list"}',
        b'{"op":"list",NaN}',
        b'{"op":"run","action":"a","args":[NaN]}',
    ],
)
def test_bad_requests(raw: bytes) -> None:
    with pytest.raises(ProtocolError):
        parse_request(raw)


def test_too_long() -> None:
    pad = "x" * MAX_LINE
    raw = ('{"op":"list","pad":"' + pad + '"}').encode()
    assert len(raw) > MAX_LINE
    with pytest.raises(ProtocolError):
        parse_request(raw)


def test_at_limit_ok() -> None:
    head = b'{"op":"list","pad":"'
    tail = b'"}'
    raw = head + b"x" * (MAX_LINE - len(head) - len(tail)) + tail
    assert len(raw) == MAX_LINE
    assert parse_request(raw) == ListReq()


def test_invalid_utf8_is_protocol_error() -> None:
    with pytest.raises(ProtocolError):
        parse_request(b'{"op":"run","action":"\xff","args":[]}')


def test_encode() -> None:
    out = encode({"a": 1, "b": ["x", None], "c": "é"})
    assert out == '{"a":1,"b":["x",null],"c":"é"}\n'.encode()
    assert out.count(b"\n") == 1
    assert out.endswith(b"\n")


def test_pending_obj() -> None:
    assert pending_obj(None) == {"status": "pending"}
    assert "tag" not in pending_obj(None)
    assert pending_obj("abcd") == {"status": "pending", "tag": "abcd"}


def roundtrip(obj: Obj) -> Obj:
    return data.as_obj(data.loads(encode(obj)))


def test_response_listed() -> None:
    info = ActionInfo("a", "desc", ("^x$", "^y$"), ("i1", "i2"), "granted", "3 min left")
    obj = roundtrip(response_obj(Listed((info,))))
    assert obj == {
        "status": "ok",
        "actions": [
            {
                "name": "a",
                "description": "desc",
                "args": ["^x$", "^y$"],
                "items": ["i1", "i2"],
                "state": "granted",
                "detail": "3 min left",
            }
        ],
    }


def test_response_listed_empty() -> None:
    assert roundtrip(response_obj(Listed(()))) == {"status": "ok", "actions": []}


def test_response_ran() -> None:
    obj = roundtrip(response_obj(Ran(3, b"\x00\xffout", b"err", True)))
    assert obj == {
        "status": "ok",
        "exit": 3,
        "stdout": data.b64(b"\x00\xffout"),
        "stderr": data.b64(b"err"),
        "truncated": True,
    }
    assert data.unb64(data.get_str(obj, "stdout")) == b"\x00\xffout"


def test_response_exported() -> None:
    profiles = {"p": {"K": "v"}, "q": {}}
    assert roundtrip(response_obj(Exported(profiles))) == {"status": "ok", "profiles": profiles}


def test_response_denied() -> None:
    assert roundtrip(response_obj(Denied("unknown action"))) == {
        "status": "denied",
        "reason": "unknown action",
    }


def test_response_failed() -> None:
    assert roundtrip(response_obj(Failed("boom"))) == {"status": "error", "reason": "boom"}


@pytest.mark.parametrize(
    "raw",
    [
        b'{"op":"list","x":' + b"1" * 5000 + b"}",
        b'{"op":"list","x":' + b"[" * 100000,
        b'{"op":"list","x":1e999}',
        b"1" * 5000,
        b"[" * 100000,
    ],
)
def test_hostile_request_lines(raw: bytes) -> None:
    with pytest.raises(ProtocolError):
        parse_request(raw)
