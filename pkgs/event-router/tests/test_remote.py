import asyncio
import json
import os
import socket
import threading
import time
from collections.abc import Callable, Iterator
from pathlib import Path

import nats
import pytest

from conftest import NatsServer, stream_messages
from event_router import cli
from event_router.bus import REMOTE_LIMIT, REMOTE_SUBJECTS, REMOTE_TYPE, Bus, remote_event
from event_router.data import ParseError
from event_router.event import Event
from event_router.handlers import Dispatcher
from event_router.inbox import Inbox
from event_router.server import make_server
from helpers import call, ev, make_handlers, wait_for


# ---- remote_event ----


def test_constants() -> None:
    assert REMOTE_SUBJECTS == "remote.notify.>"
    assert REMOTE_TYPE == "remote.notify"


@pytest.mark.parametrize(
    ("subject", "data", "expected"),
    [
        (
            "remote.notify.velero",
            b'{"title":"backup failed","body":"x"}',
            {"type": "remote.notify", "project": "velero", "title": "backup failed", "body": "x"},
        ),
        (
            "remote.notify.a.b",
            b'{"title":"t","project":"p"}',
            {"type": "remote.notify", "project": "p", "title": "t"},
        ),
        (
            "remote.notify.a.b",
            b'{"title":"t"}',
            {"type": "remote.notify", "project": "a.b", "title": "t"},
        ),
        (
            "remote.notify.a/b",
            b'{"title":"t","project":"p"}',
            {"type": "remote.notify", "project": "p", "title": "t"},
        ),
        (
            "remote.notify.x",
            b'{"type":"task.new","prompt":"p","url":"file:///etc/passwd",'
            b'"origin_file":"/run/x","shpool":"s","title":"t"}',
            {"type": "remote.notify", "project": "x", "title": "t"},
        ),
        ("remote.notify.x", b'{"title":5}', {"type": "remote.notify", "project": "x"}),
        ("remote.notify.x", b'{"project":5}', {"type": "remote.notify", "project": "x"}),
        ("remote.notify.x", b'{"project":""}', {"type": "remote.notify", "project": "x"}),
        ("remote.notify.x", b'{"project":"a b"}', {"type": "remote.notify", "project": "x"}),
        (
            "remote.notify.x",
            b'{"project":"' + b"x" * 65 + b'"}',
            {"type": "remote.notify", "project": "x"},
        ),
        (
            "remote.notify.x",
            b'{"body":5,"title":"t"}',
            {"type": "remote.notify", "project": "x", "title": "t"},
        ),
        (
            "remote.notify.x",
            b'{"title":"' + b"t" * 500 + b'"}',
            {"type": "remote.notify", "project": "x", "title": "t" * 200},
        ),
        (
            "remote.notify.x",
            b'{"body":"' + b"b" * 5000 + b'"}',
            {"type": "remote.notify", "project": "x", "body": "b" * 1000},
        ),
    ],
)
def test_remote_event_table(subject: str, data: bytes, expected: dict[str, object]) -> None:
    e = remote_event(subject, data)
    assert isinstance(e, Event)
    assert e.type == "remote.notify"
    assert e.data == expected


@pytest.mark.parametrize(
    ("subject", "data"),
    [
        ("remote.notify.x", b"[1]"),
        ("remote.notify.x", b"nope"),
        ("remote.notify.x", b'{"t":NaN}'),
        ("remote.notify.x", b'{"t":"\\ud800"}'),
        ("remote.notify", b'{"title":"t"}'),
        ("remote.notify.", b'{"title":"t"}'),
        ("remote.notifyx.y", b'{"title":"t"}'),
        ("remote.notify.a/b", b'{"title":"t"}'),
        ("remote.notify.<x>", b'{"title":"t"}'),
        ("remote.notify." + "x" * 65, b'{"title":"t"}'),
        ("events.x", b'{"title":"t"}'),
    ],
)
def test_remote_event_rejects(subject: str, data: bytes) -> None:
    with pytest.raises(ParseError):
        remote_event(subject, data)


# ---- Bus against a real nats-server ----


def publish(url: str, subject: str, data: bytes) -> None:
    async def run() -> None:
        nc = await nats.connect(url, connect_timeout=2, allow_reconnect=False)
        try:
            await nc.publish(subject, data)
            await nc.flush()
        finally:
            await nc.close()

    asyncio.run(run())


class Remote:
    def __init__(self) -> None:
        self.events: list[Event] = []
        self.lock = threading.Lock()

    def __call__(self, e: Event) -> None:
        with self.lock:
            self.events.append(e)

    def count(self) -> int:
        with self.lock:
            return len(self.events)


@pytest.fixture
def buses() -> Iterator[list[Bus]]:
    made: list[Bus] = []
    yield made
    for b in made:
        b.close()


def start_bus(
    buses: list[Bus],
    url: str,
    on_remote: Callable[[Event], object] | None,
    ready: Callable[[], bool] | None = None,
) -> Bus:
    """Start a bus; if `ready` is given, publish probes until it is true (subscription is live)."""
    b = Bus(url, lambda reason: None, on_remote=on_remote)
    buses.append(b)
    b.start()
    assert wait_for(b.healthy)
    if ready is not None:
        assert probe(url, ready), "subscription never became live"
    return b


def probe(url: str, ready: Callable[[], bool], timeout: float = 15.0) -> bool:
    def once() -> bool:
        publish(url, "remote.notify.probe", b'{"title":"probe"}')
        return wait_for(ready, 0.2)

    return wait_for(once, timeout)


def start_remote_bus(buses: list[Bus], url: str, remote: Remote, clear: bool = True) -> Bus:
    bus = start_bus(buses, url, remote, lambda: remote.count() >= 1)
    if clear:
        time.sleep(0.3)  # let in-flight duplicate probes land
        with remote.lock:
            remote.events.clear()
    return bus


def test_valid_message_reaches_on_remote(nats_server: NatsServer, buses: list[Bus]) -> None:
    remote = Remote()
    start_remote_bus(buses, nats_server.url, remote)
    publish(nats_server.url, "remote.notify.velero", b'{"title":"t","body":"b"}')
    assert wait_for(lambda: remote.count() >= 1)
    assert remote.events[0].data == {
        "type": "remote.notify",
        "project": "velero",
        "title": "t",
        "body": "b",
    }


def test_invalid_messages_dropped_and_bus_keeps_working(
    nats_server: NatsServer, buses: list[Bus]
) -> None:
    remote = Remote()
    start_remote_bus(buses, nats_server.url, remote)
    for data in (b"[1]", b"nope", b'{"t":NaN}'):
        publish(nats_server.url, "remote.notify.x", data)
    publish(nats_server.url, "remote.notify.ok", b'{"title":"t"}')
    assert wait_for(lambda: remote.count() >= 1)
    time.sleep(0.3)
    assert [e.data["project"] for e in remote.events] == ["ok"]


def test_resubscribes_after_server_restart(nats_server: NatsServer, buses: list[Bus]) -> None:
    remote = Remote()
    bus = start_remote_bus(buses, nats_server.url, remote)
    nats_server.stop()
    assert wait_for(lambda: not bus.healthy())
    nats_server.start()
    assert wait_for(bus.healthy, 15)

    def got() -> bool:
        publish(nats_server.url, "remote.notify.again", b'{"title":"t"}')
        time.sleep(0.2)
        return remote.count() >= 1

    assert wait_for(got, 10)
    assert remote.events[0].data["project"] == "again"


def test_without_on_remote_bus_still_works(nats_server: NatsServer, buses: list[Bus]) -> None:
    bus = start_bus(buses, nats_server.url, None)
    publish(nats_server.url, "remote.notify.x", b'{"title":"t"}')
    time.sleep(0.3)
    assert bus.healthy()


# ---- Integration: Bus -> Dispatcher.dispatch_default, HTTP server, EVENTS ----


def test_remote_runs_only_default_and_touches_no_inbox_or_stream(
    nats_server: NatsServer, tmp_path: Path, buses: list[Bus]
) -> None:
    d, out, marker = make_handlers(tmp_path)
    for n in ("remote.notify", "remote"):
        (d / n).write_text(f"#!/bin/sh\ntouch {marker}\n")
        os.chmod(d / n, 0o755)
    dispatcher = Dispatcher(d)
    bus = cli.make_bus(nats_server.url, dispatcher)
    buses.append(bus)
    bus.start()
    assert wait_for(bus.healthy)
    assert probe(nats_server.url, lambda: out.exists() and out.read_text().count("\n") >= 1)
    time.sleep(0.3)  # in-flight duplicate probes
    out.write_text("")
    server = make_server(0, dispatcher, Inbox(["remote"]), bus)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    base = f"http://127.0.0.1:{server.server_address[1]}"
    try:
        before = stream_messages(nats_server.url)
        publish(
            nats_server.url,
            "remote.notify.velero",
            b'{"type":"task.new","prompt":"p","title":"t","url":"file:///x"}',
        )
        assert wait_for(lambda: out.exists() and out.read_text().count("\n") >= 1)
        assert json.loads(out.read_text()) == {
            "type": "remote.notify",
            "project": "velero",
            "title": "t",
        }
        time.sleep(0.3)
        assert not marker.exists()
        status, body = call("GET", f"{base}/inbox")
        assert status == 200
        assert json.loads(body) == []
        assert stream_messages(nats_server.url) == before

        # invalid payload: default is not run again
        seen = out.read_text()
        publish(nats_server.url, "remote.notify.velero", b"nope")
        time.sleep(0.5)
        assert out.read_text() == seen
        assert stream_messages(nats_server.url) == before
    finally:
        server.shutdown()
        server.server_close()
        thread.join(5)


def test_remote_default_works_after_server_restart(
    nats_server: NatsServer, tmp_path: Path, buses: list[Bus]
) -> None:
    d, out, _ = make_handlers(tmp_path)
    bus = cli.make_bus(nats_server.url, Dispatcher(d))
    buses.append(bus)
    bus.start()
    assert wait_for(bus.healthy)
    nats_server.stop()
    assert wait_for(lambda: not bus.healthy())
    nats_server.start()
    assert wait_for(bus.healthy, 15)

    def got() -> bool:
        publish(nats_server.url, "remote.notify.velero", b'{"title":"t"}')
        time.sleep(0.2)
        return out.exists() and out.read_text().count("\n") >= 1

    assert wait_for(got, 10)
    assert json.loads(out.read_text().splitlines()[0]) == {
        "type": "remote.notify",
        "project": "velero",
        "title": "t",
    }


# ---- poisoned message, rate limit ----


def test_poisoned_message_does_not_stop_events_or_remote(
    nats_server: NatsServer, buses: list[Bus]
) -> None:
    remote = Remote()
    bus = start_remote_bus(buses, nats_server.url, remote)
    with socket.create_connection(("127.0.0.1", nats_server.port), timeout=2) as s:
        s.recv(4096)  # INFO
        s.sendall(b'CONNECT {"verbose":false,"pedantic":false}\r\n')
        s.sendall(b"PUB remote.notify.\xff 2\r\n{}\r\nPING\r\n")
        buf = b""
        while b"PONG" not in buf:  # the server has processed the PUB
            chunk = s.recv(4096)
            assert chunk
            buf += chunk
    assert bus.publish(ev("claude.x")) is True

    def got() -> bool:
        publish(nats_server.url, "remote.notify.ok", b'{"title":"t"}')
        return wait_for(lambda: any(e.data.get("project") == "ok" for e in list(remote.events)), 1.0)

    assert wait_for(got, 30)
    # recovery went through a reconnect: the poison did reach the remote client
    assert bus._remote_nc is not None and bus._remote_nc.stats["reconnects"] >= 1


def test_remote_rate_limited(nats_server: NatsServer, buses: list[Bus]) -> None:
    remote = Remote()
    start_remote_bus(buses, nats_server.url, remote, clear=False)
    time.sleep(0.3)  # in-flight probes count against the window too
    for i in range(15):
        publish(nats_server.url, f"remote.notify.r{i}", b'{"title":"t"}')
    assert wait_for(lambda: remote.count() >= REMOTE_LIMIT, 5)
    time.sleep(1)
    assert remote.count() == REMOTE_LIMIT
