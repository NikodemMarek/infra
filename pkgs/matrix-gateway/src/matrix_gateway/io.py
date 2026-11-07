"""The real Matrix client (urllib) and the real NATS calls (natscli subprocesses)."""

import http.client
import subprocess
import urllib.error
import urllib.parse
import urllib.request
from collections.abc import Iterator
from email.message import Message
from typing import IO, cast

from .data import Json, Obj, ParseError, dumps, get_list, get_obj, get_str, loads
from .gateway import BusError, Fatal, MatrixError

CALL_TIMEOUT = 30
SYNC_TIMEOUT = 35
PUBLISH_TIMEOUT = 5


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
    def __init__(self, homeserver: str) -> None:
        self._base = homeserver.rstrip("/") + "/_matrix/client/v3"
        self._token: str | None = None
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
        headers: dict[str, str] = {}
        if self._token is not None:
            headers["Authorization"] = f"Bearer {self._token}"
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
            if e.code == 401 or (e.code == 403 and path == "/login"):
                raise Fatal(f"{method} {path}: unauthorized (HTTP {e.code})") from e
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

    def login(self, user: str, password: str, device_id: str) -> str:
        body: Obj = {
            "type": "m.login.password",
            "identifier": {"type": "m.id.user", "user": user},
            "password": password,
            "device_id": device_id,
            "initial_device_display_name": "matrix-gateway",
        }
        result = self._call("POST", "/login", body=body)
        self._token = _field(result, "access_token", "login")
        return _field(result, "user_id", "login")

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


class NatsBus:
    def __init__(self, url: str) -> None:
        self._url = url

    def publish(self, subject: str, data: str) -> None:
        try:
            done = subprocess.run(
                ["nats", "--server", self._url, "pub", "--no-templates", subject],
                input=data,
                capture_output=True,
                encoding="utf-8",
                errors="replace",
                timeout=PUBLISH_TIMEOUT,
            )
        except (OSError, subprocess.SubprocessError) as e:
            raise BusError(f"nats pub: {e}") from e
        if done.returncode != 0:
            raise BusError(done.stderr.strip() or f"exit {done.returncode}")


class NatsSub:
    def __init__(self, url: str) -> None:
        self._url = url

    def __iter__(self) -> Iterator[str]:
        with subprocess.Popen(
            ["nats", "--server", self._url, "sub", "--raw", "matrix.send"],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
        ) as proc:
            stdout: IO[bytes] | None = proc.stdout
            if stdout is None:
                return
            for raw in stdout:
                yield raw.decode("utf-8", "replace").rstrip("\n")
