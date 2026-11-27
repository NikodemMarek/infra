"""Pure logic: the wire format, what to post for a request and how to seal a command."""

import base64
import re
import unicodedata
from dataclasses import dataclass

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

from .data import Obj, ParseError, dumps, loads

COMMAND_HOST_RE = r"![A-Za-z0-9][A-Za-z0-9-]{0,62}"
BODY_MAX = 20000
POST_ROOMS = ("mail", "alerts")


@dataclass(frozen=True)
class Send:
    body: str
    reply_to: str | None = None
    room: str | None = None


def sanitize(text: str) -> str:
    return "".join(
        "�"
        if unicodedata.category(c) in ("Cc", "Cf", "Zl", "Zp") and c not in "\n\t"
        else c
        for c in text
    )


def parse_send(line: str) -> Send | None:
    try:
        obj = loads(line)
    except ParseError:
        return None
    if not isinstance(obj, dict):
        return None
    try:
        dumps(obj).encode()
    except (UnicodeEncodeError, RecursionError):
        return None
    body = obj.get("body")
    if not isinstance(body, str) or not body or len(body) > BODY_MAX:
        return None
    if "room" in obj:
        room = obj["room"]
        if not isinstance(room, str) or room not in POST_ROOMS:
            return None
        return Send(body, room=room)
    if "reply_to" in obj:
        reply_to = obj["reply_to"]
        if (
            not isinstance(reply_to, str)
            or not reply_to.startswith("$")
            or len(reply_to) > 255
        ):
            return None
        return Send(body, reply_to=reply_to)
    return None


def content(send: Send) -> Obj:
    body = sanitize(send.body)
    if send.reply_to is not None:
        return {
            "msgtype": "m.notice",
            "body": body,
            "m.relates_to": {"m.in_reply_to": {"event_id": send.reply_to}},
        }
    return {"msgtype": "m.text", "body": body}


def seal_command(
    key: Ed25519PrivateKey, to: str, text: str, event_id: str, ts: int
) -> str:
    fields: Obj = {
        "kind": "matrix.command",
        "to": to,
        "text": text,
        "event_id": event_id,
        "ts": ts,
    }
    msg = dumps(fields, sort_keys=True)
    sig = base64.b64encode(key.sign(msg.encode("utf-8"))).decode()
    return dumps({"type": "matrix.command", "msg": msg, "sig": sig})


def parse_command(body: str) -> tuple[str, str] | None:
    parts = body.split(maxsplit=1)
    if len(parts) != 2 or re.fullmatch(COMMAND_HOST_RE, parts[0]) is None:
        return None
    return parts[0][1:].lower(), parts[1].strip()
