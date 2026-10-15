from pathlib import Path

import pytest

from knowledge import locks
from knowledge.locks import FileLock, LockTimeout


def test_lock_path(tmp_path: Path):
    assert locks.memory_lock() == tmp_path / "run" / "knowledge" / "memory.lock"


def test_a_lock_excludes_others_until_released(tmp_path: Path):
    path = tmp_path / "locks" / "a.lock"
    first = FileLock(path)
    assert first.acquire()
    assert not FileLock(path, 0).acquire()  # a second fd on the same file
    first.release()
    second = FileLock(path)
    assert second.acquire()
    second.release()


def test_the_lock_dir_is_private(tmp_path: Path):
    with FileLock(locks.memory_lock()):
        assert locks.memory_lock().parent.stat().st_mode & 0o077 == 0
        assert locks.memory_lock().stat().st_mode & 0o077 == 0


def test_waiting_for_a_lock_times_out(tmp_path: Path):
    path = tmp_path / "a.lock"
    with FileLock(path):
        with pytest.raises(LockTimeout):
            with FileLock(path, 0.05, poll=0.01):
                pass
    with FileLock(path, 0.05, poll=0.01):
        pass


def test_the_lock_is_released_when_the_block_raises(tmp_path: Path):
    path = tmp_path / "a.lock"
    with pytest.raises(RuntimeError):
        with FileLock(path):
            raise RuntimeError("boom")
    assert FileLock(path).acquire()
