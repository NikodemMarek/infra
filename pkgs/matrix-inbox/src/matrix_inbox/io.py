"""The real Matrix client (urllib) and the real agent-inbox calls (subprocess)."""

import http.client
import subprocess
import urllib.error
import urllib.parse
import urllib.request
from email.message import Message
from typing import IO, cast

from .bot import Fatal, LocalError, MatrixError
from .data import Json, Obj, ParseError, dumps, get_list, get_obj, get_str, loads

CALL_TIMEOUT = 30
SYNC_TIMEOUT = 35
LOCAL_TIMEOUT = 10


class _NotFound(MatrixError):
    pass


def _quote(part: str) -> str:
    return urllib.parse.quote(part, safe="")


def _objects(values: list[Json] | None) -> list[Obj]:
    return [v for v in values or [] if isinstance(v, dict)]


def _field(result: Obj, key: str, what: str) -> str:
    value = get_str(result, key)
    if value is None:
        raise MatrixError(f"{what}: no {key} in the response")
    return value


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(
        self,
        req: urllib.request.Request,
        fp: IO[bytes],
        code: int,
        msg: str,
        headers: Message,
        newurl: str,
    ) -> None:
        return None


class HttpMatrix:
    def __init__(self, homeserver: str, token: str) -> None:
        self._base = homeserver.rstrip("/") + "/_matrix/client/v3"
        self._token = token
        self._opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), _NoRedirect)

    def _call(
        self,
        method: str,
        path: str,
        query: dict[str, str] | None = None,
        body: Obj | None = None,
        timeout: int = CALL_TIMEOUT,
    ) -> Obj:
        url = self._base + path + ("?" + urllib.parse.urlencode(query) if query else "")
        headers = {"Authorization": f"Bearer {self._token}"}
        data: bytes | None = None
        if body is not None:
            headers["Content-Type"] = "application/json"
            data = dumps(body).encode()
        request = urllib.request.Request(url, data=data, headers=headers, method=method)
        try:
            response = cast(http.client.HTTPResponse, self._opener.open(request, timeout=timeout))
            with response:
                raw = response.read()
        except urllib.error.HTTPError as e:
            if e.code == 401:
                raise Fatal(f"{method} {path}: unauthorized (HTTP 401)") from e
            if e.code == 404:
                raise _NotFound(f"{method} {path}: not found") from e
            raise MatrixError(f"{method} {path}: HTTP {e.code}") from e
        except (OSError, http.client.HTTPException) as e:
            raise MatrixError(f"{method} {path}: {e}") from e
        try:
            result = loads(raw)
        except ParseError as e:
            raise MatrixError(f"{method} {path}: {e}") from e
        if not isinstance(result, dict):
            raise MatrixError(f"{method} {path}: response is not an object")
        return result

    def whoami(self) -> str:
        return _field(self._call("GET", "/account/whoami"), "user_id", "whoami")

    def resolve_alias(self, alias: str) -> str | None:
        try:
            result = self._call("GET", "/directory/room/" + _quote(alias))
        except _NotFound:
            return None
        return _field(result, "room_id", "resolve alias")

    def create_room(self, alias_localpart: str, invite: str) -> str:
        body: Obj = {
            "preset": "private_chat",
            "visibility": "private",
            "room_alias_name": alias_localpart,
            "name": "Agents",
            "invite": [invite],
            "initial_state": [{"type": "m.room.retention", "state_key": "", "content": {"max_lifetime": 259200000}}],
        }
        return _field(self._call("POST", "/createRoom", body=body), "room_id", "create room")

    def recent_messages(self, room: str, sender: str, limit: int) -> list[Obj]:
        room_filter: Obj = {"types": ["m.room.message"], "senders": [sender]}
        query = {"dir": "b", "limit": str(limit), "filter": dumps(room_filter)}
        result = self._call("GET", f"/rooms/{_quote(room)}/messages", query)
        return _objects(get_list(result, "chunk"))

    def sync(self, room: str, since: str | None, timeout_ms: int) -> tuple[str, list[Obj]]:
        sync_filter: Obj = {
            "room": {
                "rooms": [room],
                "timeline": {"types": ["m.room.message"], "limit": 100},
                "state": {"types": []},
                "ephemeral": {"types": []},
                "account_data": {"types": []},
            },
            "presence": {"types": []},
            "account_data": {"types": []},
        }
        query = {"timeout": str(timeout_ms), "filter": dumps(sync_filter)}
        if since is not None:
            query["since"] = since
        result = self._call("GET", "/sync", query, timeout=SYNC_TIMEOUT)
        joined = get_obj(get_obj(result, "rooms") or {}, "join") or {}
        timeline = get_obj(get_obj(joined, room) or {}, "timeline") or {}
        return _field(result, "next_batch", "sync"), _objects(get_list(timeline, "events"))

    def get_event(self, room: str, event_id: str) -> Obj | None:
        try:
            return self._call("GET", f"/rooms/{_quote(room)}/event/{_quote(event_id)}")
        except _NotFound:
            return None

    def send(self, room: str, content: Obj, txn: str, event_type: str = "m.room.message") -> str:
        path = f"/rooms/{_quote(room)}/send/{_quote(event_type)}/{_quote(txn)}"
        return _field(self._call("PUT", path, body=content), "event_id", "send")


def _agent_inbox(args: list[str], stdin: str | None = None) -> tuple[int, str, str]:
    done = subprocess.run(
        ["agent-inbox", *args],
        input=stdin,
        capture_output=True,
        encoding="utf-8",
        errors="replace",
        timeout=LOCAL_TIMEOUT,
    )
    return done.returncode, done.stdout, done.stderr.strip() or f"exit {done.returncode}"


class ProcessLocal:
    def items(self) -> list[Obj]:
        try:
            rc, out, err = _agent_inbox(["list", "--json"])
        except (OSError, subprocess.SubprocessError) as e:
            raise LocalError(f"agent-inbox list: {e}") from e
        if rc != 0:
            raise LocalError(f"agent-inbox list: {err}")
        items: list[Obj] = []
        for line in out.splitlines():
            if not line.strip():
                continue
            try:
                item = loads(line)
            except ParseError as e:
                raise LocalError(f"agent-inbox list: {e}") from e
            if not isinstance(item, dict):
                raise LocalError("agent-inbox list: line is not an object")
            items.append(item)
        return items

    def reply(self, item_id: str, line: str) -> str | None:
        try:
            rc, _, err = _agent_inbox(["reply", item_id], line + "\n")
        except (OSError, subprocess.SubprocessError) as e:
            return f"agent-inbox reply: {e}"
        return None if rc == 0 else err
