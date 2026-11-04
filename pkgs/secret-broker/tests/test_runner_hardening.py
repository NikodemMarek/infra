"""S2 hardening of runner.run_action: cancel before spawn, escaped pipe holders."""

import sys
import threading
import time
from pathlib import Path

from secret_broker.runner import run_action


def test_cancel_already_set_never_spawns(tmp_path: Path) -> None:
    marker = tmp_path / "marker"
    cancel = threading.Event()
    cancel.set()
    argv = [sys.executable, "-c", f"open({str(marker)!r}, 'w').close()"]
    r = run_action(argv, {}, 20, cancel)
    time.sleep(0.5)  # a child that was spawned would have written it by now
    assert not marker.exists()
    assert r.cancelled


def test_setsid_child_holding_stdout_is_overflow_and_returns_in_time() -> None:
    code = (
        "import os, sys, time\n"
        "if os.fork() == 0:\n"
        "    os.setsid()\n"
        "    sys.stdout.write('x'); sys.stdout.flush()\n"
        "    time.sleep(30)\n"
        "    os._exit(0)\n"
        "sys.stdout.write('leader'); sys.stdout.flush()\n"
    )
    timeout = 5
    start = time.monotonic()
    r = run_action([sys.executable, "-c", code], {}, timeout, threading.Event())
    assert time.monotonic() - start < timeout + 2
    assert r.overflow is True
