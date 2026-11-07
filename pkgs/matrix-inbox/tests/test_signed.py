import base64

import pytest
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey, Ed25519PublicKey

from matrix_inbox.data import Json, dumps
from matrix_inbox.signed import Reply, load_key, open_reply

FIELDS: dict[str, Json] = {"to": "yenn", "ref": "/r/x/reply", "text": "y", "body": "B", "event_id": "$e1"}


def msg_of(fields: dict[str, Json]) -> str:
    return dumps(fields, sort_keys=True)


def envelope(msg: str, sig: str, **extra: Json) -> str:
    return dumps({"type": "matrix.reply", "msg": msg, "sig": sig, **extra})


def sign(key: Ed25519PrivateKey, msg: str) -> str:
    return base64.b64encode(key.sign(msg.encode("utf-8"))).decode()


def sealed(key: Ed25519PrivateKey, fields: dict[str, Json] | None = None) -> str:
    msg = msg_of(FIELDS if fields is None else fields)
    return envelope(msg, sign(key, msg))


def raw_public(key: Ed25519PrivateKey) -> bytes:
    return key.public_key().public_bytes(serialization.Encoding.Raw, serialization.PublicFormat.Raw)


def test_valid_reply_opens() -> None:
    key = Ed25519PrivateKey.generate()
    assert open_reply(sealed(key), key.public_key()) == Reply("yenn", "/r/x/reply", "y", "B", "$e1")


def test_valid_reply_with_unicode_and_newlines() -> None:
    key = Ed25519PrivateKey.generate()
    fields: dict[str, Json] = {**FIELDS, "text": "ż\nó", "body": "l1\nl2 é — \"q\""}
    assert open_reply(sealed(key, fields), key.public_key()) == Reply("yenn", "/r/x/reply", "ż\nó", "l1\nl2 é — \"q\"", "$e1")


def test_extra_keys_ignored() -> None:
    key = Ed25519PrivateKey.generate()
    msg = msg_of({**FIELDS, "extra": 1})
    line = envelope(msg, sign(key, msg), other="x")
    assert open_reply(line, key.public_key()) == Reply("yenn", "/r/x/reply", "y", "B", "$e1")


def test_wrong_key() -> None:
    signer = Ed25519PrivateKey.generate()
    other = Ed25519PrivateKey.generate()
    assert open_reply(sealed(signer), other.public_key()) is None


def test_tampered_msg() -> None:
    key = Ed25519PrivateKey.generate()
    msg = msg_of(FIELDS)
    sig = sign(key, msg)
    tampered = msg.replace('"text":"y"', '"text":"z"')
    assert tampered != msg
    assert open_reply(envelope(tampered, sig), key.public_key()) is None


def test_sig_not_base64() -> None:
    key = Ed25519PrivateKey.generate()
    assert open_reply(envelope(msg_of(FIELDS), "not base64!!"), key.public_key()) is None


def test_sig_of_another_message() -> None:
    key = Ed25519PrivateKey.generate()
    assert open_reply(envelope(msg_of(FIELDS), sign(key, "other")), key.public_key()) is None


def test_empty_sig() -> None:
    key = Ed25519PrivateKey.generate()
    assert open_reply(envelope(msg_of(FIELDS), ""), key.public_key()) is None


def test_msg_missing_field() -> None:
    key = Ed25519PrivateKey.generate()
    fields = {k: v for k, v in FIELDS.items() if k != "event_id"}
    assert open_reply(sealed(key, fields), key.public_key()) is None


@pytest.mark.parametrize("field", list(FIELDS))
def test_msg_field_not_a_string(field: str) -> None:
    key = Ed25519PrivateKey.generate()
    assert open_reply(sealed(key, {**FIELDS, field: 1}), key.public_key()) is None


def test_msg_not_an_object() -> None:
    key = Ed25519PrivateKey.generate()
    msg = "[1]"
    assert open_reply(envelope(msg, sign(key, msg)), key.public_key()) is None


def test_msg_not_json() -> None:
    key = Ed25519PrivateKey.generate()
    msg = "nil body"
    assert open_reply(envelope(msg, sign(key, msg)), key.public_key()) is None


def test_msg_or_sig_not_a_string() -> None:
    key = Ed25519PrivateKey.generate()
    sig = sign(key, msg_of(FIELDS))
    assert open_reply(dumps({"type": "matrix.reply", "msg": FIELDS, "sig": sig}), key.public_key()) is None
    assert open_reply(dumps({"type": "matrix.reply", "msg": msg_of(FIELDS), "sig": 5}), key.public_key()) is None
    assert open_reply(dumps({"type": "matrix.reply", "msg": msg_of(FIELDS)}), key.public_key()) is None
    assert open_reply(dumps({"type": "matrix.reply", "sig": sig}), key.public_key()) is None


@pytest.mark.parametrize("line", ["nil body", "", "[1]", '"s"', "5", "null", "{", '{"msg":'])
def test_malformed_line(line: str) -> None:
    key = Ed25519PrivateKey.generate()
    assert open_reply(line, key.public_key()) is None


def test_wrong_type_is_rejected() -> None:
    key = Ed25519PrivateKey.generate()
    msg = msg_of(FIELDS)
    line = dumps({"type": "other", "msg": msg, "sig": sign(key, msg)})
    assert open_reply(line, key.public_key()) is None


def test_load_key_roundtrip() -> None:
    key = Ed25519PrivateKey.generate()
    loaded = load_key(base64.b64encode(raw_public(key)).decode())
    assert isinstance(loaded, Ed25519PublicKey)
    assert open_reply(sealed(key), loaded) == Reply("yenn", "/r/x/reply", "y", "B", "$e1")


@pytest.mark.parametrize("text", ["!!!not base64", "AAAA", "", base64.b64encode(b"x" * 31).decode(), base64.b64encode(b"x" * 33).decode()])
def test_load_key_invalid(text: str) -> None:
    with pytest.raises(ValueError):
        load_key(text)
