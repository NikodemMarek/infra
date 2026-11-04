"""Run an action's command in its own process group with a fresh HOME and a fixed environment."""

import os
import shutil
import signal
import subprocess
import tempfile
import threading
import time
from collections.abc import Callable, Iterator, Mapping, Sequence
from contextlib import contextmanager, suppress
from pathlib import Path
from typing import IO

from .broker import OUTPUT_CAP
from .model import RunResult

FIXED_PATH = "/run/current-system/sw/bin"
CERTS = "/etc/ssl/certs/ca-certificates.crt"
KEEP = OUTPUT_CAP + 64 * 1024
SWEEP_FAILED = "could not clear the service cgroup"


@contextmanager
def child_dir() -> Iterator[Path]:
    """`tempfile.mkdtemp()` (0700) under the private /tmp, removed with `shutil.rmtree` afterwards."""
    d = Path(tempfile.mkdtemp())
    try:
        yield d
    finally:
        shutil.rmtree(d, ignore_errors=True)


def action_env(secrets: Mapping[str, str], home: Path) -> dict[str, str]:
    """The fixed keys PATH, HOME, TMPDIR, LANG, SSL_CERT_FILE, NIX_SSL_CERT_FILE, then `secrets`."""
    return {
        "PATH": FIXED_PATH,
        "HOME": str(home),
        "TMPDIR": str(home),
        "LANG": "C.UTF-8",
        "SSL_CERT_FILE": CERTS,
        "NIX_SSL_CERT_FILE": CERTS,
        **secrets,
    }


class _Capture(threading.Thread):
    """Reads a stream to EOF, keeping at most KEEP bytes."""

    def __init__(self, stream: IO[bytes]) -> None:
        super().__init__(daemon=True)
        self._stream = stream
        self.data = bytearray()
        self.overflow = False

    def run(self) -> None:
        while chunk := self._stream.read(65536):
            room = KEEP - len(self.data)
            self.data += chunk[:room]
            if len(chunk) > room:
                self.overflow = True
        self._stream.close()


def _kill_group(pgid: int) -> None:
    with suppress(ProcessLookupError):
        os.killpg(pgid, signal.SIGKILL)


def run_action(argv: Sequence[str], secrets: Mapping[str, str], timeout: int, cancel: threading.Event) -> RunResult:
    """Run `argv` in `child_dir()`; kill the whole group on timeout, cancel and after the leader exits."""
    if cancel.is_set():
        return RunResult(exit=128 + signal.SIGKILL, stdout=b"", stderr=b"", cancelled=True)
    with child_dir() as home:
        proc = subprocess.Popen(
            argv,
            env=action_env(secrets, home),
            cwd=home,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            start_new_session=True,
            close_fds=True,
        )
        assert proc.stdout is not None and proc.stderr is not None
        out, err = _Capture(proc.stdout), _Capture(proc.stderr)
        deadline = time.monotonic() + timeout
        timed_out = cancelled = False
        try:
            out.start()
            err.start()
            while proc.poll() is None:
                if cancel.wait(0.1):
                    cancelled = True
                    break
                if time.monotonic() >= deadline:
                    timed_out = True
                    break
        finally:
            _kill_group(proc.pid)
            rc = proc.wait()
        # a process that left the group can keep the pipes open: take what was read so far
        out.join(1)
        err.join(1)
        alive = out.is_alive() or err.is_alive()
        return RunResult(
            exit=128 - rc if rc < 0 else rc,
            stdout=bytes(out.data),
            stderr=bytes(err.data),
            timed_out=timed_out,
            cancelled=cancelled,
            overflow=out.overflow or err.overflow or alive,
        )


def sweep(
    procs: Path,
    own_pid: int,
    kill: Callable[[int, int], None] = os.kill,
    sleep: Callable[[float], None] = time.sleep,
    rounds: int = 10,
    wait: float = 0.05,
) -> bool:
    """SIGKILL every pid in `procs` except `own_pid`; True once only `own_pid` (or nothing) is left."""
    for _ in range(rounds):
        strays = [pid for pid in map(int, procs.read_text().split()) if pid != own_pid]
        if not strays:
            return True
        for pid in strays:
            with suppress(ProcessLookupError):
                kill(pid, signal.SIGKILL)
        sleep(wait)
    return False


def own_cgroup_procs() -> Path:
    """`/sys/fs/cgroup` + the path of the `0::` line of /proc/self/cgroup + `/cgroup.procs`."""
    for line in Path("/proc/self/cgroup").read_text().splitlines():
        if line.startswith("0::"):
            return Path("/sys/fs/cgroup") / line[3:].lstrip("/") / "cgroup.procs"
    raise OSError("no cgroup v2 entry in /proc/self/cgroup")
