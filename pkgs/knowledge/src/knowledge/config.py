"""Paths and environment flags: the one place that reads the environment."""

import os
from pathlib import Path


class UsageError(Exception):
    """Bad input from the user or the environment (exit 2)."""


def _env(name: str) -> str | None:
    """A variable that is set and not empty."""
    return os.environ.get(name) or None


# paths


def memory_dir() -> Path:
    if (v := _env("CLAUDE_MEMORY_DIR")) is not None:
        return Path(v)
    return claude_config_dir() / "memory"


def claude_config_dir() -> Path:
    if (v := _env("CLAUDE_CONFIG_DIR")) is not None:
        return Path(v)
    return Path.home() / ".local" / "share" / "claude"


def state_dir() -> Path:
    """Not created here: `ensure_private` does that, for whoever writes."""
    base = _env("XDG_STATE_HOME")
    return (Path(base) if base else Path.home() / ".local" / "state") / "knowledge"


def inbox_dir() -> Path:
    return state_dir() / "inbox"


def runtime_dir() -> Path:
    """The shell autocommit hook uses the same path."""
    base = _env("XDG_RUNTIME_DIR")
    return (Path(base) if base else Path("/tmp") / f"user-{os.getuid()}") / "knowledge"


def claude_bin() -> str:
    return _env("KNOWLEDGE_CLAUDE") or "claude-bare"


def ensure_private(path: Path) -> Path:
    """Create the directory (mode 0700) if missing; returns it."""
    path.mkdir(parents=True, exist_ok=True, mode=0o700)
    return path


# flags


def origin() -> str | None:
    """Set by the event launcher, e.g. `event:gitlab.note`."""
    return _env("KNOWLEDGE_ORIGIN")


def in_claude_code() -> bool:
    """Claude Code sets CLAUDECODE in the environment of the commands an agent runs."""
    return _env("CLAUDECODE") is not None


def curator_active() -> bool:
    """Inside a curator run: hooks do nothing."""
    return os.environ.get("KNOWLEDGE_CURATOR") == "1"
