"""Verification of the gateway's signed replies."""

import base64
from dataclasses import dataclass

from cryptography.exceptions import InvalidSignature
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey

from .data import ParseError, get_str, loads


@dataclass(frozen=True)
class Reply:
    to: str
    ref: str
    text: str
    body: str
    event_id: str


def load_key(b64: str) -> Ed25519PublicKey:
    raw = base64.b64decode(b64, validate=True)
    if len(raw) != 32:
        raise ValueError("an Ed25519 public key is 32 bytes")
    return Ed25519PublicKey.from_public_bytes(raw)


def _verified(line: str, key: Ed25519PublicKey) -> str | None:
    envelope = loads(line)
    if not isinstance(envelope, dict):
        return None
    msg = get_str(envelope, "msg")
    sig = get_str(envelope, "sig")
    if get_str(envelope, "type") != "matrix.reply" or msg is None or sig is None:
        return None
    try:
        key.verify(base64.b64decode(sig, validate=True), msg.encode("utf-8"))
    except (InvalidSignature, ValueError):
        return None
    return msg


def open_reply(line: str, key: Ed25519PublicKey) -> Reply | None:
    try:
        msg = _verified(line, key)
        if msg is None:
            return None
        fields = loads(msg)
    except (ParseError, RecursionError):
        return None
    if not isinstance(fields, dict):
        return None
    to = get_str(fields, "to")
    ref = get_str(fields, "ref")
    text = get_str(fields, "text")
    body = get_str(fields, "body")
    event_id = get_str(fields, "event_id")
    if to is None or ref is None or text is None or body is None or event_id is None:
        return None
    return Reply(to, ref, text, body, event_id)
