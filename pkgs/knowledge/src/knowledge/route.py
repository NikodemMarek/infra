"""`knowledge route`: one model call, without tools, that picks the repository a task belongs in."""

from collections.abc import Iterable, Sequence
from dataclasses import dataclass
from pathlib import Path

from . import data
from .config import memory_dir
from .context import ProjectRow, project_for_cwd, projects_index
from .data import ParseError
from .llm import ClaudeCli, Llm, LlmError
from .prompts import router_prompt

MODEL = "haiku"
EFFORT = "low"
TIMEOUT = 60.0  # seconds
DESCRIPTION_CHARS = 300  # of the description of one repository
README_BYTES = 8192
_SKIPPED_STARTS = ("#", "![", "[![", "<")  # headings, images and badges, html
_FENCES = ("```", "~~~")  # what is between two of them is code
_UNDERLINE = frozenset("=-~")  # setext and rst heading underlines


@dataclass(frozen=True)
class Answer:
    repo: str | None  # the name of a candidate; None: no clear choice
    reason: str


def _brief(lines: Iterable[str]) -> str:
    """The lines as one line of at most DESCRIPTION_CHARS."""
    text = " ".join(" ".join(lines).split())
    return text if len(text) <= DESCRIPTION_CHARS else text[: DESCRIPTION_CHARS - 3].rstrip() + "..."


def _readme_rank(p: Path) -> tuple[int, str]:
    return len(p.name), p.name


def readme_head(repo: Path) -> str:
    """The first text of the README of the repo, without headings, badges, code blocks and underlines; "" if none."""
    try:
        # the shortest name: README.md, not a translation like README-zh.md
        readme = min(
            (p for p in repo.iterdir() if p.name.lower().startswith("readme") and p.is_file()),
            key=_readme_rank,
        )
        with readme.open("rb") as f:
            raw = f.read(README_BYTES)
    except (OSError, ValueError):  # ValueError: min() of no README
        return ""
    kept: list[str] = []
    fenced = False
    for raw_line in raw.decode("utf-8", "replace").splitlines():
        line = raw_line.strip()
        if line.startswith(_FENCES):
            fenced = not fenced
        elif not fenced and line and not line.startswith(_SKIPPED_STARTS) and not set(line) <= _UNDERLINE:
            kept.append(line)
    return _brief(kept)


def purpose(memory: Path, slug: str) -> str:
    """The `## Purpose` section of the overview of the project in the memory; "" if none."""
    try:
        text = (memory / "projects" / slug / "overview.md").read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError):
        return ""
    lines: list[str] = []
    inside = False
    for line in text.splitlines():
        if line.startswith("## "):
            if inside:
                break
            inside = line[3:].strip().lower() == "purpose"
        elif inside:
            lines.append(line)
    return _brief(lines)


def describe(repo: Path, memory: Path, rows: Sequence[ProjectRow]) -> str:
    """What the repo is: the purpose of its project in the memory, else the head of its README, else ""."""
    row = project_for_cwd(repo, rows)
    return (purpose(memory, row.slug) if row is not None else "") or readme_head(repo)


def message(task: str, candidates: Sequence[Path], memory: Path) -> str:
    """What is sent on stdin: the task, verbatim, and one line per candidate."""
    rows = projects_index(memory)
    lines: list[str] = []
    for repo in candidates:
        description = describe(repo, memory, rows)
        lines.append(f"- {repo.name}: {description}" if description else f"- {repo.name}")
    return "\n".join(["Task:", task, "", "Repositories (name: what it is):", *lines])


def parse(text: str, names: Sequence[str]) -> Answer:
    """The answer in the model's text (a JSON object, maybe fenced or among prose). Raises LlmError if it is none."""
    start = text.find("{")
    shown = repr(text.strip()[:200])
    if start < 0:
        raise LlmError(f"model answered {shown}, not a JSON object")
    try:
        obj = data.as_obj(data.loads_from(text, start), "answer")
        repo = data.opt_str(obj, "repo")
        reason = data.str_or(obj, "reason")
    except ParseError as e:
        raise LlmError(f"model answered {shown}: {e}") from e
    if repo is None or repo.strip().lower() in ("", "null", "none"):
        return Answer(None, reason)
    if repo not in names:
        raise LlmError(f"model answered {repo!r}, not a candidate ({', '.join(names)})")
    return Answer(repo, reason)


def route(task: str, candidates: Sequence[Path], llm: Llm | None = None) -> Answer:
    """The repo among the candidates the task belongs in. Raises LlmError if the call or its answer fails."""
    msg = message(task, candidates, memory_dir())
    llm = llm or ClaudeCli(model=MODEL, effort=EFFORT, timeout=TIMEOUT, tools="")
    return parse(llm.run(router_prompt(), msg, []), [p.name for p in candidates])
