import json
import threading
import urllib.error
import urllib.request
from collections.abc import Iterator

import pytest

from event_router.handlers import Dispatcher
from event_router.inbox import Inbox
from event_router.server import make_server


@pytest.fixture
def base() -> Iterator[str]:
    # no handlers dir: nothing is ever dispatched, so only queued events are accepted
    server = make_server(0, Dispatcher(None), Inbox(["claude"]))
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f"http://127.0.0.1:{server.server_address[1]}"
    finally:
        server.shutdown()
        server.server_close()
        thread.join(5)


def call(method: str, url: str, body: object = None) -> tuple[int, bytes]:
    data = body if isinstance(body, bytes) else None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(url, data=data, method=method)
    try:
        with urllib.request.urlopen(req, timeout=5) as r:
            return r.status, r.read()
    except urllib.error.HTTPError as e:
        return e.code, e.read()


def inbox(base: str) -> list[dict]:
    status, body = call("GET", f"{base}/inbox")
    assert status == 200
    return json.loads(body)


def test_queued_event_is_listed_and_deletable(base: str) -> None:
    assert call("POST", f"{base}/events", {"type": "claude.x", "title": "hi"})[0] == 202
    (item,) = inbox(base)
    assert item["event"] == {"type": "claude.x", "title": "hi"}
    assert isinstance(item["received"], float)

    assert call("DELETE", f"{base}/inbox/{item['id']}") == (204, b"")
    assert call("DELETE", f"{base}/inbox/{item['id']}")[0] == 404
    assert inbox(base) == []


def test_event_with_same_key_replaces_the_queued_one(base: str) -> None:
    for title in ("a", "b"):
        assert call("POST", f"{base}/events", {"type": "claude.waiting", "key": "s1", "title": title})[0] == 202
    (item,) = inbox(base)
    assert item["event"]["title"] == "b"


def test_unqueued_event_without_handler_is_500(base: str) -> None:
    assert call("POST", f"{base}/events", {"type": "other.x"})[0] == 500
    assert inbox(base) == []


def test_bad_event_is_400(base: str) -> None:
    assert call("POST", f"{base}/events", {"title": "no type"})[0] == 400
    assert inbox(base) == []


def test_unencodable_or_non_standard_json_is_400(base: str) -> None:
    # a lone surrogate would poison every later GET /inbox; NaN is not valid JSON for clients
    assert call("POST", f"{base}/events", b'{"type":"claude.x","t":"\\ud800"}')[0] == 400
    assert call("POST", f"{base}/events", b'{"type":"claude.x","t":NaN}')[0] == 400
    assert call("POST", f"{base}/events", b'{"type":"claude.x","t":-Infinity}')[0] == 400
    assert inbox(base) == []


def test_unknown_paths_are_404(base: str) -> None:
    assert call("GET", f"{base}/nope")[0] == 404
    assert call("DELETE", f"{base}/nope")[0] == 404
    assert call("DELETE", f"{base}/inbox/")[0] == 404


def test_health(base: str) -> None:
    assert call("GET", f"{base}/health")[0] == 200
