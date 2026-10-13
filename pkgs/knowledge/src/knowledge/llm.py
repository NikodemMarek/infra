"""Running `claude -p` as the curator."""

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
DISALLOWED_TOOLS = ",".join(("Bash", "Edit", "Write", "NotebookEdit", "WebFetch", "WebSearch", "Task", *PRIVATE_DENY))
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


def build_argv(binary: str, system: str, add_dirs: Sequence[Path], model: str, effort: str) -> list[str]:
    """The command line. --add-dir takes several values, so it comes last; the message is on stdin."""
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
        "--allowedTools",
        ALLOWED_TOOLS,
        "--disallowedTools",
        DISALLOWED_TOOLS,
        "--strict-mcp-config",
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
    """The real `claude`: read-only tools, no hooks, no MCP servers, run in the memory dir."""

    def __init__(
        self,
        binary: str | None = None,
        model: str | None = None,
        effort: str | None = None,
        timeout: float | None = None,
    ) -> None:
        t = tunables()
        self.binary = binary or claude_bin()
        self.model = model or t.model
        self.effort = effort or t.effort
        self.timeout = float(t.timeout) if timeout is None else timeout

    def run(self, system: str, message: str, add_dirs: Sequence[Path]) -> str:
        argv = build_argv(self.binary, system, add_dirs, self.model, self.effort)
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
        except ParseError as e:
            raise LlmError(f"unexpected claude output: {e}") from e
        if is_error:
            raise LlmError(f"claude reported an error ({subtype}): {_tail(result or '')}")
        if result is None:
            raise LlmError("claude's output has no result")
        return result
