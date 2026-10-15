"""What a session starts with: the scope files that apply to its directory (SCOPES.toml, INDEX.md) within a size cap.

A scope is `topic/jj` (the file `topic/jj.md`, likewise `lang/` and `org/`) or a project (`projects/<slug>/facts.md`).
global.md is not injected here: the deployed CLAUDE.md imports it.
"""

import datetime
import os
import re
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path

from . import config, data, notes
from .data import ParseError

SCOPES_FILE = "SCOPES.toml"
MAX_BYTES = 9500  # of the injected text: Claude Code turns a hook's additionalContext over 10,000 chars into a file
STALE_AFTER = datetime.timedelta(days=1)
HEADER = "Curated knowledge for this directory (global.md comes with CLAUDE.md):"

_SCOPE = re.compile(r"(?:lang|topic|org)/[a-z0-9][a-z0-9-]{0,63}")
_SLUG = re.compile(r"[a-z0-9][a-z0-9-]{0,63}")
_SEPARATOR_CELL = re.compile(r":?-+:?")
_AGENTS = ".agents"


@dataclass(frozen=True)
class ProjectRow:
    """One row of the architect's `INDEX.md` table."""

    slug: str
    status: str
    repo: str  # repo path


@dataclass(frozen=True)
class PathRule:
    prefix: str  # may start with ~
    scopes: tuple[str, ...]


@dataclass(frozen=True)
class MarkerRule:
    file: str  # a file or directory name
    scopes: tuple[str, ...]


# SCOPES.toml and INDEX.md


def projects_index(memory: Path) -> list[ProjectRow]:
    """Rows of the `INDEX.md` table (slug, status, repo path); none if the file is missing."""
    try:
        text = (memory / "INDEX.md").read_text(encoding="utf-8")
    except (FileNotFoundError, UnicodeDecodeError):
        return []
    rows: list[ProjectRow] = []
    for line in text.splitlines():
        line = line.strip()
        if not line.startswith("|"):
            continue
        cells = [c.strip() for c in line.strip("|").split("|")]
        if len(cells) < 3 or cells[0].lower() == "slug":
            continue
        if all(_SEPARATOR_CELL.fullmatch(c) for c in cells):
            continue
        slug, status, repo = cells[0], cells[1], cells[2].strip("`").strip()
        if slug and repo:
            rows.append(ProjectRow(slug, status, repo))
    return rows


def load_rules(memory: Path) -> tuple[list[PathRule], list[MarkerRule]]:
    """The `[[path]]` and `[[marker]]` rules of SCOPES.toml (any other key is ignored); none without the file.

    Raises ParseError if the file is not valid.
    """
    try:
        text = (memory / SCOPES_FILE).read_text(encoding="utf-8")
    except FileNotFoundError:
        return [], []
    except UnicodeDecodeError as e:
        raise ParseError(f"{SCOPES_FILE}: not UTF-8 text") from e
    try:
        doc = data.load_toml(text)
        paths = [
            PathRule(data.get_str(o, "prefix"), tuple(data.str_list(o, "scopes")))
            for o in (data.as_obj(i, "path") for i in data.opt_list(doc, "path"))
        ]
        markers = [
            MarkerRule(data.get_str(o, "file"), tuple(data.str_list(o, "scopes")))
            for o in (data.as_obj(i, "marker") for i in data.opt_list(doc, "marker"))
        ]
    except ParseError as e:
        raise ParseError(f"{SCOPES_FILE}: {e}") from e
    return paths, markers


# scopes for a directory


def normalise_cwd(cwd: Path) -> Path:
    """A jj workspace `X.agents/<ws>/...` is `X/...` (the agents' workspaces stand for their repo)."""
    parts = Path(os.path.abspath(cwd)).parts
    out: list[str] = []
    i = 0
    while i < len(parts):
        part = parts[i]
        if part.endswith(_AGENTS) and part != _AGENTS and i + 1 < len(parts):
            out.append(part.removesuffix(_AGENTS))
            i += 2
        else:
            out.append(part)
            i += 1
    return Path(*out)


def repo_root(cwd: Path) -> Path:
    """The nearest ancestor (or cwd itself) with `.jj` or `.git`; the cwd if there is none."""
    for d in (cwd, *cwd.parents):
        if (d / ".jj").exists() or (d / ".git").exists():
            return d
    return cwd


def _under(path: Path, prefix: Path) -> bool:
    return path == prefix or prefix in path.parents


def project_for_cwd(cwd: Path, projects: Sequence[ProjectRow]) -> ProjectRow | None:
    """The project whose repo is the longest prefix of `cwd` (normalised: an agents' workspace is its repo)."""
    cwd = normalise_cwd(cwd)
    best: ProjectRow | None = None
    best_depth = -1
    for row in projects:
        repo = Path(row.repo).expanduser()
        if _under(cwd, repo) and len(repo.parts) > best_depth:
            best, best_depth = row, len(repo.parts)
    return best


def _order(scope: str) -> tuple[int, str]:
    """Org first, then lang and topic."""
    return (0 if scope.startswith("org/") else 1), scope


def scope_files(cwd: Path, memory: Path) -> list[str]:
    """The scope files (paths relative to the memory dir, which may not exist) that apply in `cwd`:
    the project's, org, then lang and topic. Raises ParseError if SCOPES.toml is not valid."""
    best = project_for_cwd(cwd, projects_index(memory))
    cwd = normalise_cwd(cwd)
    root = repo_root(cwd)
    paths, markers = load_rules(memory)
    found: set[str] = set()
    for rule in paths:
        if _under(cwd, Path(rule.prefix).expanduser()):
            found.update(rule.scopes)
    for marker in markers:
        if (root / marker.file).exists() or (cwd / marker.file).exists():
            found.update(marker.scopes)
    files = [f"{s}.md" for s in sorted(found, key=_order) if _SCOPE.fullmatch(s) is not None]
    if best is not None and _SLUG.fullmatch(best.slug) is not None:
        files.insert(0, f"projects/{best.slug}/facts.md")
    return files


# the injected text


def _size(text: str) -> int:
    return len(text.encode("utf-8"))


def _join(memory: Path, blocks: Sequence[tuple[str, str]], kept: Sequence[int]) -> str:
    shown = [blocks[i][1] for i in kept]
    also = [f"also relevant: {memory / rel}" for i, (rel, _) in enumerate(blocks) if i not in kept]
    return "\n".join([HEADER, *shown, *also]) + "\n"


def session_context(cwd: Path) -> str | None:
    """The header, then each existing scope file as `## <relpath>` and its text, at most MAX_BYTES; the files that
    do not fit are listed as `also relevant: <path>`. None if no scope file exists."""
    memory = config.memory_dir()
    blocks: list[tuple[str, str]] = []
    for rel in scope_files(cwd, memory):
        try:
            text = (memory / rel).read_text(encoding="utf-8", errors="replace").strip()
        except OSError:
            continue
        if text:
            blocks.append((rel, f"## {rel}\n{text}\n"))
    if not blocks:
        return None
    kept: list[int] = []
    used = _size(HEADER) + 1
    for i, (_, block) in enumerate(blocks):
        if used + _size(block) + 1 <= MAX_BYTES:
            kept.append(i)
            used += _size(block) + 1
    while True:  # the `also relevant` lines take room too
        out = _join(memory, blocks, kept)
        if _size(out) <= MAX_BYTES or not kept:
            return out
        kept.pop()


def stale_inbox_line(now: datetime.datetime | None = None) -> str | None:
    """A line for the user when notes have waited for the curator for more than a day."""
    oldest = notes.oldest_date()
    if oldest is None or (now or datetime.datetime.now(datetime.UTC)) - oldest <= STALE_AFTER:
        return None
    return (
        f"{len(notes.pending())} knowledge notes wait in {config.inbox_dir()} since {oldest:%Y-%m-%d}; "
        "check: systemctl --user status knowledge-curate"
    )
