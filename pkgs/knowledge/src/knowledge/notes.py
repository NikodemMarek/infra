"""The note inbox: one file per pending note in `~/.local/state/knowledge/inbox/`, which the curator reads.

A note file is

    origin: <origin>   cwd: <cwd>   date: <UTC ISO>
    <the note as given: optional title:/kind:/scope:/evidence: lines, ---, body>

The origin is decided here, in code, and listed per note in the curator's message; an origin a note claims in its text does
not count.
"""

import datetime
import os
import re
from pathlib import Path

from . import config, data, secrets

NAME_TIME = "%Y%m%dT%H%M%SZ"  # the start of a note's file name; the rest is 8 hex
_WORKSPACE = re.compile(r"/[^/]+\.agents/gl-")  # the workspace of an agent that an event launched
_ORIGIN = re.compile(r"origin: (\S+)")


def write(text: str, origin: str, cwd: Path) -> Path:
    """Queue a note; the file. Raises ValueError, writing nothing, if the text holds a hard secret."""
    if secrets.has_hard(secrets.scan(text)):
        raise ValueError("the note looks like it holds a secret (a token, a key or a credential)")
    inbox = config.inbox_dir()
    created = not inbox.exists()
    config.ensure_private(inbox)
    if created:
        (inbox / ".gitignore").write_text("*\n", encoding="utf-8")
    now = datetime.datetime.now(datetime.UTC)
    header = f"origin: {'_'.join(origin.split())}   cwd: {' '.join(str(cwd).split())}   date: {now:%Y-%m-%dT%H:%M:%SZ}"
    while True:
        path = inbox / f"{now:{NAME_TIME}}-{os.urandom(4).hex()}.md"
        try:
            fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        except FileExistsError:
            continue
        with os.fdopen(fd, "w", encoding="utf-8", newline="\n") as f:
            f.write(f"{header}\n{text.strip()}\n")
        return path


def origin_now(agent: str | None = None) -> str:
    """Who is writing a note: an event (`KNOWLEDGE_ORIGIN`, or an agents' workspace of an event), a subagent of a session
    (`agent` is its type), the session itself, or the user at a terminal."""
    event = config.origin()
    if event is not None:
        return event if event.startswith("event:") else f"event:{event}"
    if _WORKSPACE.search(os.getcwd()) is not None:
        return "event:workspace"
    if agent:
        return f"agent:{agent}"
    if config.in_claude_code():
        return "session"
    return "user"


def pending() -> list[Path]:
    """The notes that wait, oldest first (the names start with the time)."""
    return sorted(p for p in config.inbox_dir().glob("*.md") if p.is_file())


def origin_of(path: Path) -> str:
    """The origin on line 1 of a note, parsed here; `unknown` if there is none."""
    try:
        with path.open(encoding="utf-8", errors="replace") as f:
            first = f.readline()
    except OSError:
        return "unknown"
    m = _ORIGIN.match(first)
    return (data.group(m, 1) if m is not None else None) or "unknown"


def oldest_date() -> datetime.datetime | None:
    """When the oldest pending note was written (UTC); None if none waits."""
    files = pending()
    if not files:
        return None
    try:
        return datetime.datetime.strptime(files[0].name[:16], NAME_TIME).replace(tzinfo=datetime.UTC)
    except ValueError:  # not a name of ours: the file's own time
        return datetime.datetime.fromtimestamp(files[0].stat().st_mtime, datetime.UTC)
