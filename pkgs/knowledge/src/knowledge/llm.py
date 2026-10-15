"""Running a headless `claude-bare -p` as the curator."""

import logging
import os
import subprocess
from collections.abc import Sequence
from pathlib import Path
from typing import Protocol

from . import data
from .config import claude_bin, memory_dir, tunables
from .data import Obj, ParseError

log = logging.getLogger("knowledge")

ALLOWED_TOOLS = "Read,Grep,Glob"
# the private notes of the user (`private/` of the memory dir, which is the cwd) are never for the model to read
PRIVATE_DENY = ("Read(./private/**)", "Grep(./private/**)", "Glob(./private/**)")
DISALLOWED_TOOLS = ",".join(PRIVATE_DENY)
# with `edits`: the model may change the files they name. Only Edit rules are allowed (they cover Write too); no bare
# Read/Grep/Glob rule, so the reads outside the cwd are denied in `-p`
EDIT_TOOLS = "Read,Grep,Glob,Edit,Write"
MAX_DETAIL = 500  # chars of claude's output an error message carries


class LlmError(Exception):
    """The call failed."""


class Llm(Protocol):
    def run(self, system: str, message: str, add_dirs: Sequence[Path]) -> str:
        """One call: `message` on stdin, `system` appended to the system prompt; returns the answer text."""
        ...


def _tail(text: str) -> str:
    one_line = " ".join(text.split())
    return one_line if len(one_line) <= MAX_DETAIL else "..." + one_line[-MAX_DETAIL:]


def build_argv(
    binary: str,
    system: str,
    add_dirs: Sequence[Path],
    model: str,
    effort: str,
    tools: str = ALLOWED_TOOLS,
    edits: Sequence[str] = (),
) -> list[str]:
    """The command line. --tools and --add-dir take several values, so a flag follows --tools and
    --add-dir comes last; the message is on stdin (never a positional argument).
    `tools` "": no tools at all (so nothing to allow or deny).
    `edits`: absolute paths or globs the model may edit; `tools` is then EDIT_TOOLS and the allow rules are
    `Edit(//<path without its leading slash>)` only, with the permission mode given explicitly."""
    if edits:
        if not all(p.startswith("/") for p in edits):
            raise ValueError("edits must be absolute paths")
        tools = EDIT_TOOLS
        rules: tuple[str, ...] = (
            "--allowedTools",
            ",".join(f"Edit(/{p})" for p in edits),
            "--disallowedTools",
            DISALLOWED_TOOLS,
            "--permission-mode",
            "default",
        )
    else:
        rules = ("--allowedTools", tools, "--disallowedTools", DISALLOWED_TOOLS) if tools else ()
    return [
        binary,
        "-p",
        "--model",
        model,
        "--effort",
        effort,
        "--output-format",
        "json",
        "--no-session-persistence",
        "--tools",
        tools,
        *rules,
        "--strict-mcp-config",  # claude-bare already passes it; matters if KNOWLEDGE_CLAUDE is a full claude
        "--settings",
        data.dumps({"disableAllHooks": True}),
        "--append-system-prompt",
        system,
        *(arg for d in add_dirs for arg in ("--add-dir", str(d))),
    ]


def _document(text: str) -> Obj | None:
    try:
        return data.as_obj(data.loads(text), "claude output")
    except ParseError:
        return None


class ClaudeCli:
    """The real `claude-bare`: read-only tools only (and Edit on `edits`), no hooks, no MCP servers, run in the memory dir.

    With `edits`, a call in which a tool was denied fails (`permission_denials` in claude's result)."""

    def __init__(
        self,
        binary: str | None = None,
        model: str | None = None,
        effort: str | None = None,
        timeout: float | None = None,
        tools: str = ALLOWED_TOOLS,
        edits: Sequence[str] = (),
    ) -> None:
        t = tunables()
        self.binary = binary or claude_bin()
        self.model = model or t.model
        self.effort = effort or t.effort
        self.timeout = float(t.timeout) if timeout is None else timeout
        self.tools = tools
        self.edits = tuple(edits)

    def run(self, system: str, message: str, add_dirs: Sequence[Path]) -> str:
        argv = build_argv(self.binary, system, add_dirs, self.model, self.effort, self.tools, self.edits)
        # CLAUDECODE is dropped: a nested launch from inside Claude Code
        env = {k: v for k, v in os.environ.items() if k != "CLAUDECODE"} | {"KNOWLEDGE_CURATOR": "1"}
        try:
            proc = subprocess.run(
                argv,
                input=message.encode("utf-8", "replace"),
                capture_output=True,
                cwd=memory_dir(),
                env=env,
                timeout=self.timeout,
                check=False,
            )
        except subprocess.TimeoutExpired as e:
            raise LlmError(f"claude timed out after {self.timeout:g}s") from e
        except OSError as e:
            raise LlmError(f"cannot run claude: {e}") from e
        out = proc.stdout.decode("utf-8", "replace")
        if proc.returncode != 0:
            detail = _tail(proc.stderr.decode("utf-8", "replace") or out)
            raise LlmError(f"claude exited with status {proc.returncode}: {detail}")
        doc = _document(out)
        if doc is None:
            raise LlmError(f"claude did not print a JSON object: {_tail(out)}")
        try:
            is_error = data.get_bool(doc, "is_error")
            result = data.opt_str(doc, "result")
            subtype = data.str_or(doc, "subtype", "error")
            denials = data.opt_list(doc, "permission_denials") if self.edits else []
        except ParseError as e:
            raise LlmError(f"unexpected claude output: {e}") from e
        if is_error:
            raise LlmError(f"claude reported an error ({subtype}): {_tail(result or '')}")
        if result is None:
            raise LlmError("claude's output has no result")
        if denials:
            raise LlmError(f"claude was denied {len(denials)} tool call(s): {_tail(data.dumps(denials))}")
        return result
