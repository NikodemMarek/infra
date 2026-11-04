import json
import os
import time
import urllib.error
import urllib.request
from collections.abc import Callable
from pathlib import Path

from event_router.event import Event


def ev(type_: str, **payload: object) -> Event:
    return Event.from_json({"type": type_, **payload})  # type: ignore[arg-type]


def wait_for(cond: Callable[[], bool], timeout: float = 5.0) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if cond():
            return True
        time.sleep(0.05)
    return cond()


def call(method: str, url: str, body: object = None) -> tuple[int, bytes]:
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(
        url, data=data, method=method, headers={"Content-Type": "application/json"}
    )
    try:
        with urllib.request.urlopen(req, timeout=5) as r:
            return r.status, r.read()
    except urllib.error.HTTPError as e:
        return e.code, e.read()


def make_handlers(tmp_path: Path) -> tuple[Path, Path, Path]:
    d = tmp_path / "handlers"
    d.mkdir()
    out = tmp_path / "default.out"
    marker = tmp_path / "task.marker"
    (d / "default").write_text(f"#!/bin/sh\ncat >> {out}\necho >> {out}\n")
    (d / "task.new").write_text(f"#!/bin/sh\ntouch {marker}\n")
    for n in ("default", "task.new"):
        os.chmod(d / n, 0o755)
    return d, out, marker
