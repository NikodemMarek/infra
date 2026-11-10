import base64

import pytest
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

from matrix_inbox.data import Json, dumps
from matrix_inbox.signed import Command, open_command, open_reply

FIELDS: dict[str, Json] = {"kind": "matrix.command", "to": "yenn", "text": "fix it", "event_id": "$c1", "ts": 1700000000000}
REPLY_FIELDS: dict[str, Json] = {"to": "yenn", "ref": "/r/x/reply", "text": "y", "body": "B", "event_id": "$e1"}


def msg_of(fields: dict[str, Json]) -> str:
    return dumps(fields, sort_keys=True)


def sign(key: Ed25519PrivateKey, msg: str) -> str:
    return base64.b64encode(key.sign(msg.encode("utf-8"))).decode()


def envelope(msg: str, sig: str, type_: str = "matrix.command", **extra: Json) -> str:
    return dumps({"type": type_, "msg": msg, "sig": sig, **extra})


def sealed(key: Ed25519PrivateKey, fields: dict[str, Json] | None = None, type_: str = "matrix.command") -> str:
    msg = msg_of(FIELDS if fields is None else fields)
    return envelope(msg, sign(key, msg), type_)


def test_valid_command_opens() -> None:
    key = Ed25519PrivateKey.generate()
    assert open_command(sealed(key), key.public_key()) == Command("yenn", "fix it", "$c1", 1700000000000)


def test_command_with_unicode_and_newlines() -> None:
    key = Ed25519PrivateKey.generate()
    text = 'ż\nó — "q"'
    assert open_command(sealed(key, {**FIELDS, "text": text}), key.public_key()) == Command("yenn", text, "$c1", 1700000000000)


def test_extra_keys_ignored() -> None:
    key = Ed25519PrivateKey.generate()
    msg = msg_of({**FIELDS, "extra": 1})
    line = envelope(msg, sign(key, msg), other="x")
    assert open_command(line, key.public_key()) == Command("yenn", "fix it", "$c1", 1700000000000)


def test_wrong_key() -> None:
    signer = Ed25519PrivateKey.generate()
    other = Ed25519PrivateKey.generate()
    assert open_command(sealed(signer), other.public_key()) is None


def test_tampered_msg() -> None:
    key = Ed25519PrivateKey.generate()
    msg = msg_of(FIELDS)
    sig = sign(key, msg)
    tampered = msg.replace('"text":"fix it"', '"text":"rm it"')
    assert tampered != msg
    assert open_command(envelope(tampered, sig), key.public_key()) is None


@pytest.mark.parametrize("sig", ["not base64!!", ""])
def test_bad_sig(sig: str) -> None:
    key = Ed25519PrivateKey.generate()
    assert open_command(envelope(msg_of(FIELDS), sig), key.public_key()) is None


def test_sig_of_another_message() -> None:
    key = Ed25519PrivateKey.generate()
    assert open_command(envelope(msg_of(FIELDS), sign(key, "other")), key.public_key()) is None


def test_envelope_type_changed_to_reply() -> None:
    key = Ed25519PrivateKey.generate()
    assert open_command(sealed(key, type_="matrix.reply"), key.public_key()) is None


@pytest.mark.parametrize("type_", ["other", ""])
def test_envelope_type_other(type_: str) -> None:
    key = Ed25519PrivateKey.generate()
    assert open_command(sealed(key, type_=type_), key.public_key()) is None


def test_envelope_type_missing() -> None:
    key = Ed25519PrivateKey.generate()
    msg = msg_of(FIELDS)
    assert open_command(dumps({"msg": msg, "sig": sign(key, msg)}), key.public_key()) is None


def test_open_command_rejects_a_sealed_reply() -> None:
    key = Ed25519PrivateKey.generate()
    msg = msg_of(REPLY_FIELDS)
    line = envelope(msg, sign(key, msg), "matrix.reply")
    assert open_command(line, key.public_key()) is None


def test_open_reply_rejects_a_sealed_command() -> None:
    key = Ed25519PrivateKey.generate()
    assert open_reply(sealed(key), key.public_key()) is None


def test_open_reply_rejects_a_command_message_in_a_reply_envelope() -> None:
    key = Ed25519PrivateKey.generate()
    assert open_reply(sealed(key, type_="matrix.reply"), key.public_key()) is None


def test_kind_missing() -> None:
    key = Ed25519PrivateKey.generate()
    fields = {k: v for k, v in FIELDS.items() if k != "kind"}
    assert open_command(sealed(key, fields), key.public_key()) is None


@pytest.mark.parametrize("kind", ["matrix.reply", "", "other", 1, None])
def test_kind_wrong(kind: Json) -> None:
    key = Ed25519PrivateKey.generate()
    assert open_command(sealed(key, {**FIELDS, "kind": kind}), key.public_key()) is None


@pytest.mark.parametrize("field", ["to", "text", "event_id", "ts"])
def test_field_missing(field: str) -> None:
    key = Ed25519PrivateKey.generate()
    fields = {k: v for k, v in FIELDS.items() if k != field}
    assert open_command(sealed(key, fields), key.public_key()) is None


@pytest.mark.parametrize("field", ["to", "text", "event_id"])
def test_string_field_not_a_string(field: str) -> None:
    key = Ed25519PrivateKey.generate()
    assert open_command(sealed(key, {**FIELDS, field: 1}), key.public_key()) is None


@pytest.mark.parametrize("ts", ["1", True, False, 1.5, None, [1], {"a": 1}])
def test_ts_not_an_int(ts: Json) -> None:
    key = Ed25519PrivateKey.generate()
    assert open_command(sealed(key, {**FIELDS, "ts": ts}), key.public_key()) is None


def test_msg_not_an_object() -> None:
    key = Ed25519PrivateKey.generate()
    msg = "[1]"
    assert open_command(envelope(msg, sign(key, msg)), key.public_key()) is None


def test_msg_not_json() -> None:
    key = Ed25519PrivateKey.generate()
    msg = "nil body"
    assert open_command(envelope(msg, sign(key, msg)), key.public_key()) is None


def test_msg_or_sig_not_a_string_or_missing() -> None:
    key = Ed25519PrivateKey.generate()
    sig = sign(key, msg_of(FIELDS))
    assert open_command(dumps({"type": "matrix.command", "msg": FIELDS, "sig": sig}), key.public_key()) is None
    assert open_command(dumps({"type": "matrix.command", "msg": msg_of(FIELDS), "sig": 5}), key.public_key()) is None
    assert open_command(dumps({"type": "matrix.command", "msg": msg_of(FIELDS)}), key.public_key()) is None
    assert open_command(dumps({"type": "matrix.command", "sig": sig}), key.public_key()) is None


@pytest.mark.parametrize("line", ["nil body", "", "[1]", '"s"', "5", "null", "{", '{"msg":'])
def test_malformed_line(line: str) -> None:
    key = Ed25519PrivateKey.generate()
    assert open_command(line, key.public_key()) is None
