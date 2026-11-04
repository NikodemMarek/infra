"""Log accepted events to the JetStream stream EVENTS and receive `remote.notify.>`
messages. The only module that imports nats.

Stream EVENTS: subjects `events.>`, file storage, limits retention, max_age 7d,
max_bytes 256 MiB, discard old, 1 replica. It is ensured (created, or updated on
mismatch) on every connect and reconnect. Nothing consumes it.

The remote subscription (core NATS, untrusted publishers) runs on its own second
connection, so a message that breaks nats-py's read loop cannot stall EVENTS."""

import asyncio
import logging
import os
import re
import threading
import time
from collections.abc import Callable

import nats
from nats.aio.client import Client
from nats.aio.msg import Msg
from nats.aio.subscription import Subscription
from nats.js import JetStreamContext
from nats.js.api import DiscardPolicy, RetentionPolicy, StorageType, StreamConfig
from nats.js.errors import NotFoundError

from .data import Obj, ParseError, as_obj, loads
from .event import Event

log = logging.getLogger(__name__)

DEFAULT_URL = "nats://127.0.0.1:4222"
STREAM = "EVENTS"
RETRY_S = 2.0
PUBLISH_TIMEOUT = 1.0
CLOSE_TIMEOUT = 5.0
REMOTE_SUBJECTS = "remote.notify.>"
REMOTE_TYPE = "remote.notify"
REMOTE_LIMIT = 10
REMOTE_WINDOW_S = 60.0
REMOTE_PENDING = 100
REMOTE_TITLE_MAX = 200
REMOTE_BODY_MAX = 1000
_PROJECT = re.compile(r"[A-Za-z0-9._-]{1,64}")


def subject_of(type_: str) -> str:
    """`claude.question` -> `events.claude.question`."""
    return f"events.{type_}"


def remote_event(subject: str, data: bytes) -> Event:
    """Turn a message on `remote.notify.<project tokens>` into an Event of type
    `remote.notify`. Raises ParseError unless the subject starts with
    `remote.notify.` and has at least one more token, and `data` parses
    (`data.loads`) to a JSON object. The event data is `{"type": "remote.notify",
    "project": P, ...}` and keeps only `title` (cut to 200 chars) and `body` (cut
    to 1000), and only when they are strings. P is the payload `project` if it
    matches `[A-Za-z0-9._-]{1,64}`, else the subject tokens after `remote.notify.`
    joined with `.` if that matches; else ParseError."""
    prefix = REMOTE_TYPE + "."
    rest = subject.removeprefix(prefix)
    if rest == subject or not rest:
        raise ParseError(f"subject: expected {prefix}<source>, got {subject!r}")
    payload = as_obj(loads(data), "remote.notify")
    project = payload.get("project")
    if not (isinstance(project, str) and _PROJECT.fullmatch(project)):
        if not _PROJECT.fullmatch(rest):
            raise ParseError(f"subject: invalid project {rest!r}")
        project = rest
    event: Obj = {"type": REMOTE_TYPE, "project": project}
    for key, limit in (("title", REMOTE_TITLE_MAX), ("body", REMOTE_BODY_MAX)):
        value = payload.get(key)
        if isinstance(value, str):
            event[key] = value[:limit]
    return Event(REMOTE_TYPE, event)


def _stream_config() -> StreamConfig:
    return StreamConfig(
        name=STREAM,
        subjects=["events.>"],
        storage=StorageType.FILE,
        retention=RetentionPolicy.LIMITS,
        max_age=7 * 24 * 3600,
        max_bytes=256 << 20,
        discard=DiscardPolicy.OLD,
        num_replicas=1,
    )


def _key(c: StreamConfig) -> tuple[object, ...]:
    return (c.subjects, c.storage, c.retention, c.max_age, c.max_bytes, c.discard, c.num_replicas)


def _why(e: BaseException) -> str:
    return f"{type(e).__name__}: {e}"


class Bus:
    def __init__(
        self,
        url: str,
        on_down: Callable[[str], object],
        on_remote: Callable[[Event], object] | None = None,
    ) -> None:
        """`on_down(reason)` is called once per outage: on the first failed publish
        or failed ensure since start or since the last success; the next success
        re-arms it.

        `on_remote`, when set: a second nats connection (to the same url, with fast
        ping detection) subscribes (core NATS, not JetStream) to `REMOTE_SUBJECTS`
        once (nats-py resends the SUB on reconnect; at most 100 pending messages,
        the rest is dropped); each message goes through `remote_event` and
        `on_remote(event)` is called on the loop thread, at most `REMOTE_LIMIT`
        times per `REMOTE_WINDOW_S` (fixed window). Excess and invalid messages are
        dropped, with at most one warning per window. Remote events never touch the
        inbox or the stream EVENTS."""
        self._url = url
        self._on_down = on_down
        self._on_remote = on_remote
        self._lock = threading.Lock()
        self._armed = True
        self._closing = False
        self._ensured = False
        self._loop = asyncio.new_event_loop()
        self._loop.set_exception_handler(self._on_loop_error)
        self._thread: threading.Thread | None = None
        self._stop = asyncio.Event()
        self._nc: Client | None = None
        self._js: JetStreamContext | None = None
        self._main_task: asyncio.Task[None] | None = None
        self._connect_task: asyncio.Task[None] | None = None
        self._ensure_task: asyncio.Task[None] | None = None
        self._remote_nc: Client | None = None
        self._remote_task: asyncio.Task[None] | None = None
        self._sub: Subscription | None = None
        self._window_start = float("-inf")
        self._window_sent = 0
        self._window_dropped = 0

    def start(self) -> None:
        """Start one asyncio loop thread and return at once; never raises if the
        server is unreachable. nats-py retries the first connect and every later
        reconnect every 2 s forever (max_reconnect_attempts=-1); a non-connection
        error from connect (e.g. a bad URL) logs and exits the process. closed_cb
        (of either connection) exits the process (os._exit(1)) unless close() started it; an uncaught exception in the loop thread is
        logged and exits the process too."""
        self._thread = threading.Thread(target=self._run, name="bus", daemon=True)
        self._thread.start()

    def publish(self, event: Event) -> bool:
        """Connected and EVENTS ensured: JetStream publish to `subject_of(event.type)`
        with payload `event.dumps()`, wait up to 1 s for the ack, True. Disconnected:
        False immediately. No ack or JetStream error: False. Never raises. Callable
        from any thread but the loop thread."""
        nc, js = self._nc, self._js
        if nc is None or js is None or not nc.is_connected:
            self._fail("not connected to nats")
            return False
        if not self._ensured:
            self._fail(f"stream {STREAM} not ready")
            return False
        coro = js.publish(subject_of(event.type), event.dumps().encode(), timeout=PUBLISH_TIMEOUT)
        try:
            fut = asyncio.run_coroutine_threadsafe(coro, self._loop)
        except Exception as e:
            # the loop is closed (after close()); the coroutine was never scheduled
            coro.close()
            self._fail(f"publish {event.type}: {_why(e)}")
            return False
        try:
            fut.result(PUBLISH_TIMEOUT + 1)
        except Exception as e:
            fut.cancel()
            self._fail(f"publish {event.type}: {_why(e)}")
            return False
        self._succeed()
        return True

    def healthy(self) -> bool:
        """Connected and EVENTS ensured since the last (re)connect, and when
        `on_remote` is set the remote connection up with the subscription in place."""
        nc = self._nc
        if not (nc is not None and nc.is_connected and self._ensured):
            return False
        if self._on_remote is None:
            return True
        remote = self._remote_nc
        return remote is not None and remote.is_connected and self._sub is not None

    def close(self) -> None:
        """Idempotent; stop the loop thread and drain."""
        with self._lock:
            if self._closing:
                return
            self._closing = True
        thread = self._thread
        if thread is not None:
            try:
                asyncio.run_coroutine_threadsafe(self._shutdown(), self._loop).result(
                    CLOSE_TIMEOUT + 2
                )
            except Exception as e:
                log.warning("bus shutdown: %s", _why(e))
            thread.join(CLOSE_TIMEOUT)
        if not self._loop.is_running():
            self._loop.close()

    def _fail(self, reason: str) -> None:
        if self._closing:
            return  # publishes failing during shutdown are not an outage
        with self._lock:
            fire = self._armed
            self._armed = False
        if not fire:
            return
        log.error("event bus down: %s", reason)
        try:
            self._on_down(reason)
        except Exception:
            log.exception("on_down failed")

    def _succeed(self) -> None:
        with self._lock:
            self._armed = True

    def _run(self) -> None:
        try:
            asyncio.set_event_loop(self._loop)
            self._loop.run_until_complete(self._main())
        except BaseException:
            log.exception("event bus thread died")
            os._exit(1)

    def _on_loop_error(self, loop: asyncio.AbstractEventLoop, context: dict[str, object]) -> None:
        log.error("event bus loop error: %s", context.get("message"), exc_info=_exc_info(context))
        # while closing, leftovers of nats-py's teardown are expected noise
        if not self._closing:
            os._exit(1)

    async def _main(self) -> None:
        self._main_task = asyncio.current_task()
        self._connect_task = asyncio.create_task(self._connect())
        if self._on_remote is not None:
            self._remote_task = asyncio.create_task(self._connect_remote())
        await self._stop.wait()

    async def _shutdown(self) -> None:
        tasks = [
            t
            for t in (self._connect_task, self._remote_task, self._ensure_task)
            if t is not None
        ]
        for t in tasks:
            t.cancel()
        await asyncio.gather(*tasks, return_exceptions=True)
        await asyncio.gather(
            self._close_client(self._nc, "nats"),
            self._close_client(self._remote_nc, "remote nats"),
        )
        # leftover tasks would be destroyed pending when the loop stops
        keep = {asyncio.current_task(), self._main_task}
        rest = [t for t in asyncio.all_tasks() if t not in keep]
        for t in rest:
            t.cancel()
        await asyncio.gather(*rest, return_exceptions=True)
        self._stop.set()

    async def _close_client(self, nc: Client | None, name: str) -> None:
        if nc is None or nc.is_closed:
            return
        try:
            await asyncio.wait_for(nc.drain(), CLOSE_TIMEOUT)
        except Exception as e:
            log.warning("%s drain: %s", name, _why(e))
        # drain refuses while reconnecting; close() also cancels nats-py's own tasks
        if not nc.is_closed:
            try:
                await asyncio.wait_for(nc.close(), CLOSE_TIMEOUT)
            except Exception as e:
                log.warning("%s close: %s", name, _why(e))

    async def _connect(self) -> None:
        # nats-py retries the first connect forever, reconnect_time_wait apart
        try:
            nc = await nats.connect(
                servers=self._url,
                max_reconnect_attempts=-1,
                reconnect_time_wait=RETRY_S,
                error_cb=self._on_error,
                disconnected_cb=self._on_disconnected,
                reconnected_cb=self._on_reconnected,
                closed_cb=self._on_closed,
            )
        except Exception as e:
            # not a connection failure (those are retried): a config error such as a bad URL
            log.error("nats connect %s: %s", self._url, _why(e))
            os._exit(1)
        self._nc = nc
        self._js = nc.jetstream()
        log.info("connected to nats %s", self._url)
        self._start_ensure()

    async def _connect_remote(self) -> None:
        # Own connection: a malformed message kills nats-py's read loop silently, and
        # only stale pings notice, so ping fast; the main connection stays unaffected.
        try:
            nc = await nats.connect(
                servers=self._url,
                ping_interval=5,
                max_outstanding_pings=2,
                max_reconnect_attempts=-1,
                reconnect_time_wait=RETRY_S,
                error_cb=self._on_error,
                closed_cb=self._on_closed,
            )
        except Exception as e:
            log.error("nats connect %s (remote): %s", self._url, _why(e))
            os._exit(1)
        self._remote_nc = nc
        log.info("connected to nats %s (remote)", self._url)
        self._sub = await nc.subscribe(
            REMOTE_SUBJECTS, cb=self._on_remote_msg, pending_msgs_limit=REMOTE_PENDING
        )

    def _drop(self, reason: str) -> None:
        self._window_dropped += 1
        if self._window_dropped == 1:
            log.warning(
                "dropping remote messages (%s); at most one warning per %gs",
                reason,
                REMOTE_WINDOW_S,
            )

    async def _on_remote_msg(self, msg: Msg) -> None:
        assert self._on_remote is not None
        now = time.monotonic()
        if now - self._window_start >= REMOTE_WINDOW_S:
            self._window_start = now
            self._window_sent = 0
            self._window_dropped = 0
        if self._window_sent >= REMOTE_LIMIT:
            self._drop("over the limit")
            return
        try:
            event = remote_event(msg.subject, msg.data)
        except ParseError as e:
            self._drop(str(e))
            return
        self._window_sent += 1
        try:
            self._on_remote(event)
        except Exception:
            log.exception("on_remote failed for %s", msg.subject)

    async def _on_error(self, e: Exception) -> None:
        log.debug("nats error: %s", _why(e))

    async def _on_disconnected(self) -> None:
        self._ensured = False
        log.warning("disconnected from nats")

    async def _on_reconnected(self) -> None:
        self._ensured = False
        log.info("reconnected to nats")
        self._start_ensure()

    async def _on_closed(self) -> None:
        if self._closing:
            return
        log.error("nats connection closed")
        os._exit(1)

    def _start_ensure(self) -> None:
        nc, js = self._nc, self._js
        if nc is None or js is None:
            return
        if self._ensure_task is not None:
            self._ensure_task.cancel()
        self._ensure_task = asyncio.create_task(self._ensure_loop(nc, js))

    async def _ensure_loop(self, nc: Client, js: JetStreamContext) -> None:
        while not self._closing:
            try:
                await self._ensure(js)
            except Exception as e:
                # a dropped connection restarts the ensure from the reconnect callback
                if not nc.is_connected:
                    return
                self._fail(f"cannot ensure stream {STREAM}: {_why(e)}")
                await asyncio.sleep(RETRY_S)
                continue
            self._ensured = True
            self._succeed()
            return

    async def _ensure(self, js: JetStreamContext) -> None:
        want = _stream_config()
        try:
            info = await js.stream_info(STREAM)
        except NotFoundError:
            await js.add_stream(want)
            log.info("created stream %s", STREAM)
            return
        if _key(info.config) != _key(want):
            await js.update_stream(want)
            log.info("updated stream %s", STREAM)


def _exc_info(context: dict[str, object]) -> BaseException | None:
    e = context.get("exception")
    return e if isinstance(e, BaseException) else None
