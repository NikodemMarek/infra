"""Git operations on the memory repo: no hooks, and a commit takes only the paths it is given."""

import os
import subprocess
from collections.abc import Sequence
from pathlib import Path
from typing import NamedTuple

from .locks import FileLock

TIMEOUT = 120
LOCK_WAIT = 30.0  # seconds to wait for memory.lock

# variables that point git at another repo, index or object store than the one `-C` names
_FOREIGN_ENV = (
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
)


class GitError(Exception):
    def __init__(self, cmd: str, detail: str) -> None:
        super().__init__(f"git {cmd}: {detail}")
        self.detail = detail


class _Result(NamedTuple):
    code: int
    out: str
    err: str


def _message(subject: str, body: str) -> str:
    subject = " ".join(subject.split())
    if not subject:
        raise ValueError("empty commit subject")
    parts = [subject]
    if body.strip():
        parts.append(body.strip())
    return "\n\n".join(parts) + "\n"


class Git:
    def __init__(self, repo: Path, lock_path: Path, lock_timeout: float = LOCK_WAIT) -> None:
        self.repo = repo
        self.lock_path = lock_path
        self.lock_timeout = lock_timeout

    # plumbing

    def _proc(self, *args: str, timeout: float = TIMEOUT) -> _Result:
        """Run git; a non-zero exit is returned, not raised."""
        cmd = ["git", "-c", "core.hooksPath=/dev/null", "-C", str(self.repo), *args]
        shown = args[0] if args else ""
        inherited = {k: v for k, v in os.environ.items() if k not in _FOREIGN_ENV}
        env = {**inherited, "GIT_TERMINAL_PROMPT": "0", "GIT_LITERAL_PATHSPECS": "1"}
        try:
            proc = subprocess.run(
                cmd, capture_output=True, timeout=timeout, check=False, stdin=subprocess.DEVNULL, env=env
            )
        except subprocess.TimeoutExpired as e:
            raise GitError(shown, f"timed out after {timeout:g}s") from e
        except OSError as e:
            raise GitError(shown, f"cannot run git: {e}") from e
        except ValueError as e:  # an argument subprocess refuses, e.g. "embedded null byte"
            raise GitError(shown, f"invalid argument: {e}") from e
        return _Result(
            proc.returncode,
            proc.stdout.decode(errors="replace"),
            proc.stderr.decode(errors="replace"),
        )

    def run(self, *args: str) -> str:
        """stdout of `git -C repo args`; raises GitError on a non-zero exit."""
        res = self._proc(*args)
        if res.code != 0:
            msg = " ".join((res.err or res.out).split())
            raise GitError(args[0] if args else "", msg or f"exit status {res.code}")
        return res.out

    def _paths(self, paths: Sequence[str]) -> list[str]:
        """The repo-relative paths, deduplicated and checked: nothing outside the repo, nothing in `.git`."""
        for p in paths:
            parts = Path(p).parts
            if not p or Path(p).is_absolute() or ".." in parts or any(part.lower() == ".git" for part in parts):
                raise ValueError(f"not a path inside the repo: {p!r}")
        seen: dict[str, None] = {p: None for p in paths}
        return list(seen)

    def _listed(self, *args: str) -> set[str]:
        """The names a `-z` listing command prints; empty if it fails (e.g. no HEAD yet)."""
        res = self._proc(*args)
        return {n for n in res.out.split("\0") if n} if res.code == 0 else set()

    # queries

    def ensure_repo(self) -> None:
        """The repo must be a git repository of its own, not a directory inside another one."""
        top = self.run("rev-parse", "--show-toplevel").strip()
        if Path(top).resolve() != self.repo.resolve():
            raise GitError("rev-parse", f"{self.repo} is not the top of a git repository (found {top})")

    def head(self) -> str:
        return self.run("rev-parse", "HEAD").strip()

    # changes

    def _commit(self, paths: list[str], subject: str, body: str) -> str | None:
        """Commit `paths` (the caller holds the lock). Paths that are neither on disk nor known to git are dropped."""
        message = _message(subject, body)
        if not paths:
            return None
        in_index = self._listed("ls-files", "-z", "--", *paths)
        in_head = self._listed("ls-tree", "-r", "--name-only", "-z", "HEAD", "--", *paths)
        known = [p for p in paths if (self.repo / p).exists(follow_symlinks=False) or p in in_index or p in in_head]
        if not known:
            return None
        # `add` refuses a path that is gone from the index and the disk; that deletion is staged already
        addable = [p for p in known if (self.repo / p).exists(follow_symlinks=False) or p in in_index]
        if addable:
            self.run("add", "-A", "--", *addable)
        diff = self._proc("diff", "--cached", "--quiet", "--", *known)
        if diff.code == 0:
            return None
        if diff.code != 1:
            raise GitError("diff", " ".join(diff.err.split()) or f"exit status {diff.code}")
        self.run("commit", "-q", "--no-verify", "-m", message, "--", *known)
        return self.head()

    def commit(self, paths: Sequence[str], subject: str, body: str = "") -> str | None:
        """Commit only `paths`; the sha, or None if they have no changes. Waits for memory.lock."""
        checked = self._paths(paths)
        with FileLock(self.lock_path, self.lock_timeout):
            return self._commit(checked, subject, body)

    def _restore(self, paths: list[str], created: set[str]) -> None:
        """Back to HEAD (the caller holds the lock); only a path of `created` is ever deleted from the disk."""
        if not paths:
            return
        in_head = self._listed("ls-tree", "-r", "--name-only", "-z", "HEAD", "--", *paths)
        tracked = [p for p in paths if p in in_head]
        others = [p for p in paths if p not in in_head]
        if tracked:
            self.run("checkout", "HEAD", "--", *tracked)
        if others:
            self.run("rm", "--cached", "-q", "--ignore-unmatch", "--", *others)
            for p in others:
                f = self.repo / p
                if p in created and (f.is_symlink() or f.is_file()):
                    f.unlink()

    def restore(self, paths: Sequence[str], created: Sequence[str] = ()) -> None:
        """Back to HEAD: tracked paths are checked out again, the others are dropped from the index.

        A path that is not in HEAD is deleted from the disk only if it is also in `created` (the caller made it);
        any other such path stays where it is, unstaged. Waits for memory.lock.
        """
        checked = self._paths(paths)
        made = set(self._paths(created))
        with FileLock(self.lock_path, self.lock_timeout):
            self._restore(checked, made)
