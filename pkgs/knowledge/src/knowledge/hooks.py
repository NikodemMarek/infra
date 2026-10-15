"""The Claude Code hooks: the scope files for a new session, and the knowledge blocks of a finished subagent as notes (S12).

A hook must never block or break the session: `run` catches everything, and the command exits 0 whatever happens.
"""

import glob
import logging
import os
from collections.abc import Iterator
from pathlib import Path

from . import config, context, data, notes
from .blocks import SEPARATOR, Block, count_fences, parse_blocks
from .data import Json, Obj, ParseError

log = logging.getLogger("knowledge")

SESSION_START = "session-start"
SUBAGENT_STOP = "subagent-stop"
HOOKS = (SESSION_START, SUBAGENT_STOP)
DEFAULT_AGENT = "subagent"  # the origin of a block when the hook input does not name the agent type


def _opt(inp: Obj, key: str) -> str | None:
    """A field of the hook input that is a non-empty string; anything else (a missing field, another type) is None."""
    value = inp.get(key)
    return value if isinstance(value, str) and value else None


def _cwd(inp: Obj) -> Path:
    return Path(_opt(inp, "cwd") or os.getcwd())


# session start


def session_start(inp: Obj) -> Obj | None:
    """The scope files for the directory as additional context, and for the user (`systemMessage`) the notes that wait
    too long for the curator, in any directory; None if there is neither."""
    messages: list[str] = []
    text: str | None = None
    try:
        text = context.session_context(_cwd(inp))
    except Exception as e:  # e.g. SCOPES.toml is not valid: no scope files, but the user hears of it
        reason = str(e) or type(e).__name__
        log.warning("session context failed: %s", reason)
        messages.append(f"knowledge: {reason}")
    if (stale := context.stale_inbox_line()) is not None:
        messages.append(stale)
    out: Obj = {}
    if text is not None:
        out["hookSpecificOutput"] = {"hookEventName": "SessionStart", "additionalContext": text}
    if messages:
        out["systemMessage"] = "\n".join(messages)
    return out or None


# subagent stop


def _content_text(content: Json) -> str:
    """The text of a message: the string itself, or the `text` items of a list of content blocks, one per line."""
    if isinstance(content, str):
        return content
    if not isinstance(content, list):
        return ""
    parts: list[str] = []
    for item in content:
        if isinstance(item, dict) and item.get("type") == "text":
            text = item.get("text")
            if isinstance(text, str):
                parts.append(text)
    return "\n".join(parts)


def _assistant_text(line: str) -> str:
    """The text of a transcript line that is a message of the assistant; "" for any other line, or one that is not JSON."""
    try:
        obj = data.loads(line)
    except (ParseError, RecursionError):
        return ""
    if not isinstance(obj, dict) or obj.get("type") != "assistant":
        return ""
    message = obj.get("message")
    return _content_text(message.get("content")) if isinstance(message, dict) else ""


def last_assistant_text(path: Path) -> str:
    """The text of the last assistant message in a JSONL transcript that has any; "" if there is none or it cannot be read."""
    try:
        text = path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return ""
    for line in reversed(text.split("\n")):  # not splitlines(): it also splits on U+2028 and the like
        found = _assistant_text(line)
        if found.strip():
            return found
    return ""


def _transcripts(inp: Obj) -> Iterator[Path]:
    """Where the transcript of the subagent may be: the path in the input, then the subagents directory of the session.
    Never the session's own transcript: blocks there are not the subagent's."""
    if (given := _opt(inp, "agent_transcript_path")) is not None:
        yield Path(given)
    session, agent = _opt(inp, "session_id"), _opt(inp, "agent_id")
    if session is not None and agent is not None:
        pattern = f"*/{glob.escape(session)}/subagents/agent-{glob.escape(agent)}.jsonl"
        yield from sorted((config.claude_config_dir() / "projects").glob(pattern))


def agent_text(inp: Obj) -> str:
    """What the subagent said last: `last_assistant_message` if the input has it, else read from a transcript."""
    message = inp.get("last_assistant_message")
    if isinstance(message, str):
        return message
    for path in _transcripts(inp):
        text = last_assistant_text(path)
        if text:
            return text
    return ""


def _note(block: Block) -> str:
    """The text of the note of a block: its header lines, `---`, the body."""
    fields = (("title", block.title), ("kind", block.kind), ("scope", block.scope), ("evidence", block.evidence))
    return "\n".join([*(f"{key}: {value}" for key, value in fields if value), SEPARATOR, block.body])


def _summary(queued: int, refused: int, invalid: int) -> str:
    text = f"knowledge: queued {queued} note(s)"
    if refused:
        text += f"; refused {refused} block(s) that look like secrets"
    if invalid:
        text += f"; skipped {invalid} invalid block(s) (need title: and ---)"
    return text


def subagent_stop(inp: Obj) -> Obj | None:
    """Queue the knowledge blocks of the subagent's last message as notes; the message for the user, None without
    knowledge fences."""
    if inp.get("stop_hook_active") is True:  # the subagent goes on because a stop hook said so: its message was seen
        return None
    message = agent_text(inp)
    blocks = parse_blocks(message)
    invalid = count_fences(message) - len(blocks)
    if not blocks and not invalid:
        return None
    origin = notes.origin_now(_opt(inp, "agent_type") or DEFAULT_AGENT)
    cwd = _cwd(inp)
    texts: list[str] = []
    seen: set[str] = set()
    for block in blocks:  # the same block twice in a message is one note
        text = _note(block)
        if text not in seen:
            seen.add(text)
            texts.append(text)
    refused = 0
    for text in texts:
        try:
            notes.write(text, origin, cwd)
        except ValueError as e:
            log.warning("refusing a knowledge block: %s", e)
            refused += 1
    return {"systemMessage": _summary(len(texts) - refused, refused, invalid)}


# running


def _read_input() -> Obj:
    return data.as_obj(data.loads(data.read_stdin()), "hook input")


def run(name: str) -> Obj | None:
    """The JSON the hook prints, or None for nothing. Never raises, and does nothing inside a curator run."""
    if config.curator_active():
        return None
    try:
        inp = _read_input()
        return session_start(inp) if name == SESSION_START else subagent_stop(inp)
    except Exception as e:  # anything: a hook must not break the session
        reason = str(e) or type(e).__name__
        log.warning("hook %s failed: %s", name, reason)
        if name == SUBAGENT_STOP:
            return {"systemMessage": f"knowledge hook error: {reason}"}
        return None
