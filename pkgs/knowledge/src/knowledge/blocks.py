"""Knowledge blocks: the fenced sections an agent puts in its last message to submit what it learned (S12).

    ```knowledge
    title: jj absorb needs --into when trunk() has no remote bookmark
    kind: gotcha
    scope: topic/jj
    evidence: dotfiles 2026-10-06: absorb rewrote published commits
    ---
    Rule, why, example.
    ```

The fence closes at the first line that is only the same number of backticks, so a body with a fenced example (normal
triple backticks) goes in a block fenced with four (a ````knowledge ... ```` block). Either fence may be indented by up to
three spaces or tabs (a block in a list item); the lines of the block are dedented by the indent of the opening fence.
"""

import re
from dataclasses import dataclass

from . import data

_BLOCK = re.compile(r"^([ \t]{0,3})(`{3,4})knowledge[ \t]*\n(.*?)\n[ \t]{0,3}\2[ \t]*$", re.MULTILINE | re.DOTALL)
_HEADER = re.compile(r"([a-z]+):[ \t]*(.*)")
SEPARATOR = "---"
KEYS = ("title", "kind", "scope", "evidence")


@dataclass(frozen=True)
class Block:
    """The words of a block: the title and the body are not empty; the other fields are None when absent."""

    title: str
    body: str
    kind: str | None
    scope: str | None
    evidence: str | None


def _header(lines: list[str]) -> dict[str, str]:
    """The `key: value` lines; lines that are not one of KEYS are ignored, a repeated key keeps its last value."""
    found: dict[str, str] = {}
    for line in lines:
        m = _HEADER.fullmatch(line.strip())
        if m is None:
            continue
        key, value = data.group(m, 1), data.group(m, 2)
        if key is not None and key in KEYS and value is not None:
            found[key] = value.strip()
    return found


def _block(inner: str) -> Block | None:
    """The header lines up to the first `---`, then the body; None without a title, a separator or a body."""
    lines = inner.split("\n")
    stop = next((i for i, line in enumerate(lines) if line.strip() == SEPARATOR), None)
    if stop is None:
        return None
    header = _header(lines[:stop])
    title = " ".join(header.get("title", "").split())
    body = "\n".join(lines[stop + 1 :]).strip()
    if not title or not body:
        return None
    return Block(
        title=title,
        body=body,
        kind=header.get("kind") or None,
        scope=header.get("scope") or None,
        evidence=header.get("evidence") or None,
    )


def _dedent(inner: str, indent: str) -> str:
    """The lines without up to as many leading spaces or tabs as the opening fence has."""
    if not indent:
        return inner
    lines: list[str] = []
    for line in inner.split("\n"):
        cut = 0
        while cut < len(indent) and cut < len(line) and line[cut] in " \t":
            cut += 1
        lines.append(line[cut:])
    return "\n".join(lines)


def count_fences(text: str) -> int:
    """How many knowledge fences `text` has, valid blocks or not: `parse_blocks` skips the invalid ones."""
    return sum(1 for _ in _BLOCK.finditer(text.replace("\r\n", "\n")))


def parse_blocks(text: str) -> list[Block]:
    """Every knowledge block in `text`, in order; the ones that have no title, no `---` or no body are skipped."""
    found: list[Block] = []
    for m in _BLOCK.finditer(text.replace("\r\n", "\n")):
        indent, inner = data.group(m, 1), data.group(m, 3)
        block = None if indent is None or inner is None else _block(_dedent(inner, indent))
        if block is not None:
            found.append(block)
    return found
