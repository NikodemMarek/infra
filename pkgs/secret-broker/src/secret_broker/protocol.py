from . import data
from .data import Json, Obj, ParseError
from .model import (
    ActionInfo,
    Denied,
    ExportReq,
    Exported,
    Failed,
    ListReq,
    Listed,
    Ran,
    Request,
    Response,
    RunReq,
)

MAX_LINE = 1 << 20


class ProtocolError(Exception):
    pass


def _request(line: bytes) -> Request:
    o = data.as_obj(data.loads(line), "request")
    op = data.get_str(o, "op")
    cwd = data.opt_str(o, "cwd", "") or ""
    session = data.opt_str(o, "session", "") or ""
    match op:
        case "list":
            return ListReq()
        case "run":
            return RunReq(
                action=data.get_str(o, "action"),
                args=tuple(data.str_list(o.get("args"), "args")),
                cwd=cwd,
                session=session,
            )
        case "export":
            profiles = data.str_list(o.get("profiles"), "profiles")
            if not profiles:
                raise ParseError("profiles: must not be empty")
            return ExportReq(
                profiles=tuple(profiles), spec=data.get_str(o, "spec"), cwd=cwd, session=session
            )
        case _:
            raise ParseError(f"op: unknown operation {op!r}")


def parse_request(line: bytes) -> Request:
    if len(line) > MAX_LINE:
        raise ProtocolError("request too long")
    try:
        return _request(line)
    except ParseError as e:
        raise ProtocolError(str(e)) from e


def encode(obj: Obj) -> bytes:
    """One compact JSON object, newline-terminated."""
    return data.dumps(obj).encode() + b"\n"


def _info(a: ActionInfo) -> Obj:
    return {
        "name": a.name,
        "description": a.description,
        "args": [*a.args],
        "items": [*a.items],
        "state": a.state,
        "detail": a.detail,
    }


def response_obj(resp: Response) -> Obj:
    match resp:
        case Listed(actions=actions):
            return {"status": "ok", "actions": [_info(a) for a in actions]}
        case Ran():
            return {
                "status": "ok",
                "exit": resp.exit,
                "stdout": data.b64(resp.stdout),
                "stderr": data.b64(resp.stderr),
                "truncated": resp.truncated,
            }
        case Exported():
            # rebuilt, as dict[str, dict[str, str]] is not a dict[str, Json] (invariance)
            profiles: dict[str, Json] = {}
            for p, env in resp.profiles.items():
                values: dict[str, Json] = dict(env)
                profiles[p] = values
            return {"status": "ok", "profiles": profiles}
        case Denied():
            return {"status": "denied", "reason": resp.reason}
        case Failed():
            return {"status": "error", "reason": resp.reason}


def pending_obj(tag: str | None) -> Obj:
    """{"status": "pending"}, plus "tag" when not None."""
    return {"status": "pending"} if tag is None else {"status": "pending", "tag": tag}
