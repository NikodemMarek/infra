import json
import os
import threading
import time
from collections.abc import Callable, Iterator
from dataclasses import dataclass
from pathlib import Path

import pytest

from conftest import Downs, NatsServer, stream_messages
from event_router.bus import Bus
from event_router.handlers import Dispatcher
from event_router.inbox import Inbox
from event_router.server import make_server
from helpers import call, make_handlers, wait_for


@dataclass
class App:
    base: str
    bus: Bus
    out: Path  # file the `default` handler appends stdin to
    marker: Path  # written by the `task.new` handler
    handlers: Path


@pytest.fixture
def app_factory(
    tmp_path: Path, bus_factory: Callable[[str], Bus]
) -> Iterator[Callable[[str], App]]:
    servers: list[tuple[object, threading.Thread]] = []

    def make(url: str) -> App:
        d, out, marker = make_handlers(tmp_path)
        bus = bus_factory(url)
        bus.start()
        server = make_server(0, Dispatcher(d), Inbox(["claude"]), bus)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        servers.append((server, thread))
        return App(f"http://127.0.0.1:{server.server_address[1]}", bus, out, marker, d)

    yield make
    for server, thread in servers:
        server.shutdown()  # type: ignore[attr-defined]
        server.server_close()  # type: ignore[attr-defined]
        thread.join(5)


def test_accepted_event_is_published_and_inbox_filled_at_once(
    nats_server: NatsServer, app_factory: Callable[[str], App]
) -> None:
    app = app_factory(nats_server.url)
    assert wait_for(app.bus.healthy)
    event = {"type": "claude.x", "title": "hi"}
    assert call("POST", f"{app.base}/events", event)[0] == 202
    status, body = call("GET", f"{app.base}/inbox")
    assert status == 200
    assert [i["event"] for i in json.loads(body)] == [event]
    # the 202 is sent before the publish
    assert wait_for(lambda: len(stream_messages(nats_server.url)) == 1)
    ((subject, data),) = stream_messages(nats_server.url)
    assert subject == "events.claude.x"
    assert json.loads(data) == event


def test_rejected_events_are_not_published(
    nats_server: NatsServer, app_factory: Callable[[str], App]
) -> None:
    app = app_factory(nats_server.url)
    assert wait_for(app.bus.healthy)
    os.remove(app.handlers / "default")
    assert call("POST", f"{app.base}/events", {"title": "no type"})[0] == 400
    assert call("POST", f"{app.base}/events", {"type": "other.x"})[0] == 500  # not queued, no handler
    assert stream_messages(nats_server.url) == []


def test_health_follows_the_bus(
    nats_server: NatsServer, app_factory: Callable[[str], App]
) -> None:
    app = app_factory(nats_server.url)
    assert wait_for(lambda: call("GET", f"{app.base}/health")[0] == 200)
    nats_server.stop()
    assert wait_for(lambda: call("GET", f"{app.base}/health")[0] == 503)
    status, body = call("GET", f"{app.base}/health")
    assert (status, body.strip()) == (503, b"event bus down")
    nats_server.start()
    assert wait_for(lambda: call("GET", f"{app.base}/health")[0] == 200)
    status, body = call("GET", f"{app.base}/health")
    assert (status, body.strip()) == (200, b"ok")


def test_bus_outage_does_not_change_responses(
    nats_server: NatsServer, app_factory: Callable[[str], App], downs: Downs
) -> None:
    app = app_factory(nats_server.url)
    assert wait_for(app.bus.healthy)
    nats_server.stop()
    assert wait_for(lambda: not app.bus.healthy())

    for _ in range(2):
        assert call("POST", f"{app.base}/events", {"type": "other.x", "n": 1})[0] == 202
    assert wait_for(lambda: app.out.exists() and app.out.read_text().count("\n") >= 2)
    assert wait_for(lambda: downs.count() == 1)
    time.sleep(0.3)
    assert downs.count() == 1
    assert call("GET", f"{app.base}/health")[0] == 503

    nats_server.start()
    assert wait_for(lambda: call("GET", f"{app.base}/health")[0] == 200)
    assert call("POST", f"{app.base}/events", {"type": "claude.again"})[0] == 202
    assert wait_for(lambda: [s for s, _ in stream_messages(nats_server.url)] == ["events.claude.again"])

    nats_server.stop()
    assert wait_for(lambda: not app.bus.healthy())
    assert call("POST", f"{app.base}/events", {"type": "claude.y"})[0] == 202
    assert wait_for(lambda: downs.count() == 2)


def test_http_serves_while_bus_is_down_at_start_and_recovers(
    tmp_path: Path, app_factory: Callable[[str], App]
) -> None:
    server = NatsServer(tmp_path / "js")
    app = app_factory(server.url)
    try:
        assert call("POST", f"{app.base}/events", {"type": "other.x"})[0] == 202
        assert call("GET", f"{app.base}/health")[0] == 503
        server.start()
        assert wait_for(lambda: call("GET", f"{app.base}/health")[0] == 200, 8)
        assert call("POST", f"{app.base}/events", {"type": "claude.x"})[0] == 202
        assert wait_for(lambda: [s for s, _ in stream_messages(server.url)] == ["events.claude.x"])
    finally:
        server.stop()


def test_post_latency_stays_low_while_bus_is_down(
    nats_server: NatsServer, app_factory: Callable[[str], App]
) -> None:
    app = app_factory(nats_server.url)
    assert wait_for(app.bus.healthy)
    nats_server.stop()
    assert wait_for(lambda: not app.bus.healthy())
    for _ in range(3):
        t0 = time.monotonic()
        assert call("POST", f"{app.base}/events", {"type": "claude.x"})[0] == 202
        assert time.monotonic() - t0 < 0.5


def test_without_bus_health_is_ok_and_posts_unchanged(tmp_path: Path) -> None:
    d, _, _ = make_handlers(tmp_path)
    server = make_server(0, Dispatcher(d), Inbox(["claude"]))
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        base = f"http://127.0.0.1:{server.server_address[1]}"
        assert call("GET", f"{base}/health")[0] == 200
        assert call("POST", f"{base}/events", {"type": "claude.x"})[0] == 202
        assert call("POST", f"{base}/events", {"title": "x"})[0] == 400
    finally:
        server.shutdown()
        server.server_close()
        thread.join(5)


class SlowBus:
    def __init__(self) -> None:
        self.published = threading.Event()

    def healthy(self) -> bool:
        return True

    def publish(self, event: object) -> bool:
        time.sleep(1.5)
        self.published.set()
        return True


def test_reply_is_sent_before_a_slow_publish(tmp_path: Path) -> None:
    d, _, _ = make_handlers(tmp_path)
    bus = SlowBus()
    server = make_server(0, Dispatcher(d), Inbox(["claude"]), bus)  # type: ignore[arg-type]
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        base = f"http://127.0.0.1:{server.server_address[1]}"
        t0 = time.monotonic()
        assert call("POST", f"{base}/events", {"type": "claude.x"})[0] == 202
        assert time.monotonic() - t0 < 0.5
        assert bus.published.wait(5)
    finally:
        server.shutdown()
        server.server_close()
        thread.join(5)
