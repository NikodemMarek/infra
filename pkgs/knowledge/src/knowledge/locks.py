"""Lock files (in the runtime dir) and the flock that holds them."""

import fcntl
import os
import time
from pathlib import Path

from .config import runtime_dir


class LockTimeout(Exception):
    pass


def memory_lock() -> Path:
    """Held around every git commit in the memory repo; the architect autocommit hook takes it too."""
    return runtime_dir() / "memory.lock"


class FileLock:
    """An exclusive flock on a file, polled until `timeout` seconds have passed."""

    def __init__(self, path: Path, timeout: float = 0.0, poll: float = 0.2) -> None:
        self.path = path
        self.timeout = timeout
        self._poll = poll
        self._fd: int | None = None

    def acquire(self) -> bool:
        self.path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        fd = os.open(self.path, os.O_RDWR | os.O_CREAT, 0o600)
        deadline = time.monotonic() + self.timeout
        while True:
            try:
                fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                if time.monotonic() >= deadline:
                    os.close(fd)
                    return False
                time.sleep(self._poll)
            except OSError:
                os.close(fd)
                raise
            else:
                self._fd = fd
                return True

    def release(self) -> None:
        if self._fd is not None:
            os.close(self._fd)  # closing drops the flock
            self._fd = None

    def __enter__(self) -> "FileLock":
        if not self.acquire():
            raise LockTimeout(f"{self.path}: still locked after {self.timeout:g}s")
        return self

    def __exit__(self, *exc: object) -> None:
        self.release()
