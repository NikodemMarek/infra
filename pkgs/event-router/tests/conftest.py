import asyncio
import socket
import subprocess
import threading
import time
from collections.abc import Callable, Iterator
from pathlib import Path

import nats
import pytest
from nats.js.errors import NotFoundError

from event_router.bus import STREAM, Bus


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        port: int = s.getsockname()[1]
        return port


class NatsServer:
    """A JetStream nats-server that can be stopped and restarted on the same port and store."""

    def __init__(self, store: Path) -> None:
        self.port = free_port()
        self.store = store
        self.proc: subprocess.Popen[bytes] | None = None

    @property
    def url(self) -> str:
        return f"nats://127.0.0.1:{self.port}"

    def start(self) -> None:
        assert self.proc is None
        self.proc = subprocess.Popen(
            ["nats-server", "-a", "127.0.0.1", "-p", str(self.port), "-js", "-sd", str(self.store)],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            try:
                with socket.create_connection(("127.0.0.1", self.port), timeout=0.2):
                    return
            except OSError:
                time.sleep(0.05)
        raise AssertionError("nats-server did not start")

    def stop(self) -> None:
        if self.proc is not None:
            self.proc.terminate()
            self.proc.wait(10)
            self.proc = None


@pytest.fixture
def nats_server(tmp_path: Path) -> Iterator[NatsServer]:
    server = NatsServer(tmp_path / "js")
    server.start()
    try:
        yield server
    finally:
        server.stop()


# ---- nats-specific helpers ----


def _with_js(url: str, fn: Callable[..., object]) -> object:
    async def run() -> object:
        nc = await nats.connect(url, connect_timeout=2, allow_reconnect=False)
        try:
            return await fn(nc.jetstream())
        finally:
            await nc.close()

    return asyncio.run(run())


def stream_messages(url: str) -> list[tuple[str, bytes]]:
    """(subject, payload) of every message in EVENTS; [] if the stream does not exist."""

    async def f(js: object) -> list[tuple[str, bytes]]:
        try:
            info = await js.stream_info(STREAM)  # type: ignore[attr-defined]
        except NotFoundError:
            return []
        if info.state.messages == 0:
            return []
        out = []
        for seq in range(info.state.first_seq, info.state.last_seq + 1):
            try:
                m = await js.get_msg(STREAM, seq=seq)  # type: ignore[attr-defined]
            except NotFoundError:
                continue
            out.append((m.subject, m.data))
        return out

    return _with_js(url, f)  # type: ignore[return-value]


class Downs:
    def __init__(self) -> None:
        self.reasons: list[str] = []
        self.lock = threading.Lock()

    def __call__(self, reason: str) -> None:
        with self.lock:
            self.reasons.append(reason)

    def count(self) -> int:
        with self.lock:
            return len(self.reasons)


@pytest.fixture
def downs() -> Downs:
    return Downs()


@pytest.fixture
def bus_factory(downs: Downs) -> Iterator[Callable[[str], Bus]]:
    buses: list[Bus] = []

    def make(url: str) -> Bus:
        b = Bus(url, downs)
        buses.append(b)
        return b

    yield make
    for b in buses:
        b.close()
