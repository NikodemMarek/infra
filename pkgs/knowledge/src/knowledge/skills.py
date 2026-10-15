"""`knowledge skills`: one editing model call that drafts skill ideas from the memory into `skill-ideas/`.

Run by the user on demand. The model may edit only `skill-ideas/**` of the memory dir; nothing is committed (the directory is
gitignored) and nothing is notified: the paths of the drafts that were created or changed are printed.
"""

import logging
import re
from pathlib import Path

from . import config, data, prompts
from .llm import ClaudeCli, LlmError

log = logging.getLogger("knowledge")

DIR = "skill-ideas"
PROMPT = "skills.md"

_KEY = re.compile(r"([A-Za-z_-]+):[ \t]*(.*)")
_BLOCK_SCALARS = (">", "|", ">-", "|-")


def _frontmatter(text: str) -> dict[str, str]:
    """The `key: value` lines of the `---` block at the top (a continuation line is joined to its key)."""
    lines = text.splitlines()
    if not lines or lines[0].strip() != "---":
        return {}
    found: dict[str, str] = {}
    key: str | None = None
    for line in lines[1:]:
        if line.strip() == "---":
            break
        m = _KEY.fullmatch(line)
        name, value = (data.group(m, 1), data.group(m, 2)) if m is not None else (None, None)
        if name is not None and value is not None:
            key = name
            found[key] = "" if value.strip() in _BLOCK_SCALARS else value.strip().strip("\"'")
        elif key is not None and line[:1] in (" ", "\t"):
            found[key] = f"{found[key]} {line.strip()}".strip()
    return found


def deployed() -> list[str]:
    """`name: description` of each skill of the Claude config (`skills/*/SKILL.md`)."""
    lines: list[str] = []
    for path in sorted((config.claude_config_dir() / "skills").glob("*/SKILL.md")):
        try:
            fm = _frontmatter(path.read_text(encoding="utf-8", errors="replace"))
        except OSError:
            continue
        lines.append(f"- {fm.get('name') or path.parent.name}: {fm.get('description', '')}")
    return lines


def _mtimes(directory: Path) -> dict[str, int]:
    return {p.name: p.stat().st_mtime_ns for p in directory.glob("*.md") if p.is_file()}


def _message(drafts: list[str]) -> str:
    return "\n".join(
        [
            "# Deployed skills (name: description)",
            "",
            *(deployed() or ["(none)"]),
            "",
            f"# Existing drafts in {DIR}/ (improve one only with new evidence)",
            "",
            *([f"- {DIR}/{n}" for n in drafts] or ["(none)"]),
            "",
        ]
    )


def run() -> int:
    """Print the paths of the drafts the model created or changed; the exit code (1: no memory dir, or the call failed)."""
    memory = config.memory_dir()
    if not memory.is_dir():
        log.error("no memory dir at %s", memory)
        return 1
    ideas = memory / DIR
    ideas.mkdir(exist_ok=True)
    before = _mtimes(ideas)
    try:
        ClaudeCli(edits=[f"{memory.resolve()}/{DIR}/**"]).run(prompts.read(PROMPT), _message(sorted(before)), [])
    except LlmError as e:
        log.error("the skills call failed: %s", e)
        return 1
    after = _mtimes(ideas)
    for name in sorted(n for n, t in after.items() if before.get(n) != t):
        print(ideas / name)
    return 0
