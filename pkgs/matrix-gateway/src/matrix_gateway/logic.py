"""Pure logic: the wire format, what to post for a request and how to seal a reply."""

import base64
import re
import unicodedata
from dataclasses import dataclass

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

from .data import Obj, ParseError, dumps, get_obj, get_str, loads

ROUTE = "com.nkmrk.route"
TO_RE = r"[a-z0-9][a-z0-9-]{0,62}"
BODY_MAX = 20000
REF_MAX = 1024


@dataclass(frozen=True)
class Send:
    body: str
    to: str | None = None
    ref: str | None = None
    reply_to: str | None = None


def sanitize(text: str, single_line: bool = False) -> str:
    if single_line:
        text = text.replace("\n", " ").replace("\t", " ")
    return "".join(
        "�" if unicodedata.category(c) in ("Cc", "Cf", "Zl", "Zp") and c not in "\n\t" else c for c in text
    )


def strip_fallback(body: str) -> str:
    lines = body.split("\n")
    if not lines[0].startswith(">"):
        return body
    i = 0
    while i < len(lines) and lines[i].startswith(">"):
        i += 1
    if i < len(lines) and lines[i] == "":
        i += 1
    return "\n".join(lines[i:])


def _valid_to(to: str) -> bool:
    return re.fullmatch(TO_RE, to) is not None


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
    if "reply_to" in obj:
        reply_to = obj["reply_to"]
        if not isinstance(reply_to, str) or not reply_to.startswith("$") or len(reply_to) > 255:
            return None
        return Send(body, reply_to=reply_to)
    to = obj.get("to")
    ref = obj.get("ref")
    if not isinstance(to, str) or not _valid_to(to):
        return None
    if not isinstance(ref, str) or not ref or len(ref) > REF_MAX:
        return None
    return Send(body, to, ref)


def content(send: Send) -> Obj:
    body = sanitize(send.body)
    if send.reply_to is not None:
        return {
            "msgtype": "m.notice",
            "body": body,
            "m.relates_to": {"m.in_reply_to": {"event_id": send.reply_to}},
        }
    return {"msgtype": "m.text", "body": body, ROUTE: {"to": send.to, "ref": send.ref}}


def seal(key: Ed25519PrivateKey, to: str, ref: str, text: str, body: str, event_id: str) -> str:
    msg = dumps({"to": to, "ref": ref, "text": text, "body": body, "event_id": event_id}, sort_keys=True)
    sig = base64.b64encode(key.sign(msg.encode("utf-8"))).decode()
    return dumps({"type": "matrix.reply", "msg": msg, "sig": sig})


def route(parent: Obj, bot_user: str) -> tuple[str, str] | None:
    if get_str(parent, "sender") != bot_user:
        return None
    target = get_obj(get_obj(parent, "content") or {}, ROUTE)
    if target is None:
        return None
    to = get_str(target, "to")
    ref = get_str(target, "ref")
    if to is None or not _valid_to(to) or not ref:
        return None
    return to, ref
