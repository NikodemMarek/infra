import json
import os
import time
from pathlib import Path

from event_router.handlers import Dispatcher
from helpers import ev, make_handlers, wait_for


def test_dispatch_default_runs_default_with_stdin_and_env(tmp_path: Path) -> None:
    d = tmp_path / "h"
    d.mkdir()
    out = tmp_path / "out"
    (d / "default").write_text(f'#!/bin/sh\ncat > {out}\necho " $ER_TYPE" >> {out}\n')
    os.chmod(d / "default", 0o755)
    assert Dispatcher(d).dispatch_default(ev("claude.x", a=1)) is True
    assert wait_for(lambda: out.exists() and out.read_text().endswith("\n"))
    first, _, rest = out.read_text().rpartition(" ")
    assert json.loads(first) == {"type": "claude.x", "a": 1}
    assert rest.strip() == "claude.x"


def test_dispatch_default_ignores_type_named_handlers(tmp_path: Path) -> None:
    d, out, marker = make_handlers(tmp_path)
    bus_down = tmp_path / "bus_down.marker"
    (d / "event_router.bus_down").write_text(f"#!/bin/sh\ntouch {bus_down}\n")
    os.chmod(d / "event_router.bus_down", 0o755)
    dispatcher = Dispatcher(d)
    assert dispatcher.dispatch_default(ev("task.new")) is True
    assert dispatcher.dispatch_default(ev("event_router.bus_down")) is True
    assert wait_for(lambda: out.exists() and out.read_text().count("\n") >= 2)
    time.sleep(0.3)
    assert not marker.exists()
    assert not bus_down.exists()


def test_dispatch_default_without_default_is_false(tmp_path: Path) -> None:
    d = tmp_path / "h"
    d.mkdir()
    marker = tmp_path / "marker"
    (d / "task.new").write_text(f"#!/bin/sh\ntouch {marker}\n")
    os.chmod(d / "task.new", 0o755)
    assert Dispatcher(d).dispatch_default(ev("task.new")) is False
    assert Dispatcher(None).dispatch_default(ev("task.new")) is False
    time.sleep(0.3)
    assert not marker.exists()
