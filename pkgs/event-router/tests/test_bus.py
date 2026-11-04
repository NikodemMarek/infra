import json
import time
from collections.abc import Callable
from pathlib import Path

from nats.js.api import DiscardPolicy, RetentionPolicy, StorageType

from conftest import Downs, NatsServer, _with_js, free_port, stream_messages
from event_router.bus import DEFAULT_URL, STREAM, Bus, subject_of
from helpers import ev, wait_for

WEEK = 7 * 24 * 3600


# ---- test-side nats client ----


def stream_config(url: str) -> object:
    async def f(js: object) -> object:
        info = await js.stream_info(STREAM)  # type: ignore[attr-defined]
        return info.config

    return _with_js(url, f)


def add_stream(url: str, **kw: object) -> None:
    async def f(js: object) -> None:
        await js.add_stream(name=STREAM, subjects=["events.>"], **kw)  # type: ignore[attr-defined]

    _with_js(url, f)


# ---- pure ----


def test_subject_of() -> None:
    assert subject_of("claude.question") == "events.claude.question"
    assert STREAM == "EVENTS"
    assert DEFAULT_URL.startswith("nats://")


# ---- Bus against a real nats-server ----


def test_stream_is_created_with_specified_config(
    nats_server: NatsServer, bus_factory: Callable[[str], Bus]
) -> None:
    bus = bus_factory(nats_server.url)
    bus.start()
    assert wait_for(bus.healthy)
    cfg = stream_config(nats_server.url)
    assert cfg.name == STREAM  # type: ignore[attr-defined]
    assert list(cfg.subjects) == ["events.>"]  # type: ignore[attr-defined]
    assert cfg.storage == StorageType.FILE  # type: ignore[attr-defined]
    assert cfg.retention == RetentionPolicy.LIMITS  # type: ignore[attr-defined]
    assert cfg.max_age == WEEK  # type: ignore[attr-defined]
    assert cfg.max_bytes == 256 * 1024 * 1024  # type: ignore[attr-defined]
    assert cfg.discard == DiscardPolicy.OLD  # type: ignore[attr-defined]
    assert cfg.num_replicas == 1  # type: ignore[attr-defined]


def test_existing_stream_with_other_config_is_updated(
    nats_server: NatsServer, bus_factory: Callable[[str], Bus]
) -> None:
    add_stream(nats_server.url, max_age=3600)
    assert stream_config(nats_server.url).max_age == 3600  # type: ignore[attr-defined]
    bus = bus_factory(nats_server.url)
    bus.start()
    assert wait_for(bus.healthy)
    cfg = stream_config(nats_server.url)
    assert cfg.max_age == WEEK  # type: ignore[attr-defined]
    assert cfg.max_bytes == 256 * 1024 * 1024  # type: ignore[attr-defined]


def test_publish_goes_to_event_subject(
    nats_server: NatsServer, bus_factory: Callable[[str], Bus], downs: Downs
) -> None:
    bus = bus_factory(nats_server.url)
    bus.start()
    assert wait_for(bus.healthy)
    assert bus.publish(ev("claude.question", title="hi")) is True
    ((subject, data),) = stream_messages(nats_server.url)
    assert subject == "events.claude.question"
    assert json.loads(data) == {"type": "claude.question", "title": "hi"}
    assert downs.count() == 0


def test_start_returns_at_once_and_publish_is_false_without_server(
    bus_factory: Callable[[str], Bus],
) -> None:
    bus = bus_factory(f"nats://127.0.0.1:{free_port()}")
    t0 = time.monotonic()
    bus.start()
    assert time.monotonic() - t0 < 1
    assert bus.healthy() is False
    t0 = time.monotonic()
    assert bus.publish(ev("claude.x")) is False
    assert time.monotonic() - t0 < 1


def test_close_is_idempotent(nats_server: NatsServer) -> None:
    bus = Bus(nats_server.url, lambda reason: None)
    bus.start()
    assert wait_for(bus.healthy)
    bus.close()
    bus.close()


def test_publish_after_close_is_false_and_not_an_outage(nats_server: NatsServer) -> None:
    downs: list[str] = []
    bus = Bus(nats_server.url, downs.append)
    bus.start()
    assert wait_for(bus.healthy)
    bus.close()
    assert bus.publish(ev("claude.x")) is False
    assert downs == []


def test_on_down_once_per_outage_and_rearmed_after_success(
    nats_server: NatsServer, bus_factory: Callable[[str], Bus], downs: Downs
) -> None:
    bus = bus_factory(nats_server.url)
    bus.start()
    assert wait_for(bus.healthy)
    assert bus.publish(ev("claude.a")) is True

    nats_server.stop()
    assert wait_for(lambda: not bus.healthy())
    t0 = time.monotonic()
    assert bus.publish(ev("claude.b")) is False
    assert bus.publish(ev("claude.b")) is False
    assert time.monotonic() - t0 < 1
    assert downs.count() == 1

    nats_server.start()
    assert wait_for(bus.healthy)
    assert bus.publish(ev("claude.c")) is True
    assert [s for s, _ in stream_messages(nats_server.url)] == ["events.claude.a", "events.claude.c"]
    assert downs.count() == 1

    nats_server.stop()
    assert wait_for(lambda: not bus.healthy())
    assert bus.publish(ev("claude.d")) is False
    assert downs.count() == 2


def test_bus_connects_once_server_appears(
    tmp_path: Path, bus_factory: Callable[[str], Bus]
) -> None:
    server = NatsServer(tmp_path / "js")
    bus = bus_factory(server.url)
    bus.start()
    try:
        assert bus.publish(ev("claude.x")) is False
        assert not bus.healthy()
        server.start()
        assert wait_for(bus.healthy, 8)  # first connect is retried every 2 s
        assert bus.publish(ev("claude.x")) is True
        assert len(stream_messages(server.url)) == 1
    finally:
        server.stop()


def test_failed_ensure_calls_on_down_once_and_stays_unhealthy(
    nats_server: NatsServer, bus_factory: Callable[[str], Bus], downs: Downs
) -> None:
    # update_stream cannot change storage, so ensure fails
    add_stream(nats_server.url, storage=StorageType.MEMORY)
    bus = bus_factory(nats_server.url)
    bus.start()
    assert wait_for(lambda: downs.count() == 1, 4)
    time.sleep(1)
    assert downs.count() == 1
    assert bus.healthy() is False
    assert bus.publish(ev("claude.x")) is False
    assert downs.count() == 1


def test_stream_is_ensured_again_on_every_reconnect(
    nats_server: NatsServer, bus_factory: Callable[[str], Bus], tmp_path: Path
) -> None:
    bus = bus_factory(nats_server.url)
    bus.start()
    assert wait_for(bus.healthy)
    nats_server.stop()
    assert wait_for(lambda: not bus.healthy())
    nats_server.store = tmp_path / "js-fresh"  # same port, empty store: no EVENTS
    nats_server.start()

    def recreated() -> bool:
        try:
            return bool(stream_config(nats_server.url).name == STREAM)  # type: ignore[attr-defined]
        except Exception:
            return False

    assert wait_for(recreated, 8)
    assert wait_for(bus.healthy, 8)
    cfg = stream_config(nats_server.url)
    assert list(cfg.subjects) == ["events.>"]  # type: ignore[attr-defined]
    assert cfg.storage == StorageType.FILE  # type: ignore[attr-defined]
    assert cfg.max_age == WEEK  # type: ignore[attr-defined]
    assert cfg.max_bytes == 256 * 1024 * 1024  # type: ignore[attr-defined]
    assert cfg.discard == DiscardPolicy.OLD  # type: ignore[attr-defined]
    assert bus.publish(ev("claude.after")) is True
