"""Pure logic: what to post for an inbox item and how to turn a reply into an answer."""

import re
import unicodedata
from dataclasses import dataclass

from .data import Obj, dumps, get_list, get_obj, get_str

_NUMBERS = re.compile(r"\d+(\s*,\s*\d+)*")

META = "com.nkmrk.inbox"
ANSWERABLE = {"claude.permission_request", "claude.question"}
DETAIL_MAX = 2000
TITLE_MAX = 200
BODY_MAX = 16000

TOO_LONG = "Too long to approve here: reply n to deny, or answer it on the computer."
PERMISSION_HINT = "Reply y to allow or n to deny."
QUESTION_HINT = "Reply with the option number or your own answer."
QUESTIONS_HINT = "Reply with one line per question: the option number(s) or your own answer."
MULTI_HINT = "   (several: e.g. 1,3)"
REPLY_Y_OR_N = "Reply y or n."
CANNOT_ANSWER = "This question cannot be answered here."
EMPTY_ANSWER = "Empty answer."


@dataclass(frozen=True)
class Refusal:
    reason: str


def sanitize(text: str, single_line: bool = False) -> str:
    if single_line:
        text = text.replace("\n", " ").replace("\t", " ")
    return "".join(
        "�" if unicodedata.category(c) in ("Cc", "Cf", "Zl", "Zp") and c not in "\n\t" else c for c in text
    )


def _line(value: str) -> str:
    return sanitize(value, single_line=True)


def _head(value: str) -> str:
    return _line(value)[:TITLE_MAX]


def _nonempty(o: Obj, key: str) -> str | None:
    value = get_str(o, key)
    return value if value else None


def _event(item: Obj) -> Obj:
    return get_obj(item, "event") or {}


def offered(item: Obj) -> bool:
    event = _event(item)
    return get_str(event, "type") in ANSWERABLE and _nonempty(event, "reply") is not None


def claimant(items: list[Obj], reply: str) -> str | None:
    for item in items:
        if get_str(_event(item), "reply") == reply:
            return get_str(item, "id")
    return None


def detail(event: Obj) -> tuple[str, bool]:
    tool_input = get_obj(event, "tool_input") or {}
    command = get_str(tool_input, "command")
    if get_str(event, "tool_name") == "Bash" and command is not None:
        text = command
    else:
        text = dumps(tool_input, indent=2, sort_keys=True)
    text = sanitize(text)
    if len(text) <= DETAIL_MAX:
        return text, False
    return text[:DETAIL_MAX] + f"\n[{len(text) - DETAIL_MAX} more chars]", True


def _options(question: Obj) -> list[Obj]:
    return [o for o in get_list(question, "options") or [] if isinstance(o, dict) and get_str(o, "label") is not None]


def _render_permission(header: str, event: Obj, title: str) -> str:
    tool = _head(_nonempty(event, "tool_name") or "?")
    shown, truncated = detail(event)
    hint = TOO_LONG if truncated else PERMISSION_HINT
    return f"{header}: {tool}\n{title}\n\n{shown}\n\n{hint}"


def _render_question(header: str, event: Obj) -> str:
    tool_input = get_obj(event, "tool_input") or {}
    questions = [q for q in get_list(tool_input, "questions") or [] if isinstance(q, dict)]
    lines = [f"{header} asks:"]
    for i, q in enumerate(questions, 1):
        prefix = f"{i}. " if len(questions) > 1 else ""
        q_header = _nonempty(q, "header")
        label = f"{_line(q_header)}: " if q_header else ""
        lines.append(f"{prefix}{label}{sanitize(get_str(q, 'question') or '')}")
        for j, option in enumerate(_options(q), 1):
            description = _nonempty(option, "description")
            suffix = f" — {_line(description)}" if description else ""
            lines.append(f"   {j}) {_line(get_str(option, 'label') or '')}{suffix}")
        if q.get("multiSelect") is True:
            lines.append(MULTI_HINT)
    hint = QUESTIONS_HINT if len(questions) > 1 else QUESTION_HINT
    return "\n".join(lines) + f"\n\n{hint}"


def render(item: Obj) -> Obj:
    event = _event(item)
    kind = get_str(event, "type") or ""
    project = _head(_nonempty(event, "project") or "?")
    session = _nonempty(event, "session") or _nonempty(event, "shpool")
    header = project + (f" · {_head(session)}" if session else "")
    if kind == "claude.permission_request":
        body = _render_permission(header, event, _head(_nonempty(event, "title") or kind))
    else:
        body = _render_question(header, event)
    if len(body) > BODY_MAX:
        body = body[:BODY_MAX] + "\n[truncated]"
    return {"msgtype": "m.text", "body": body, META: {"id": get_str(item, "id") or "", "kind": kind}}


def strip_fallback(body: str) -> str:
    lines = body.split("\n")
    if not lines[0].startswith(">"):
        return body
    i = 0
    while i < len(lines) and lines[i].startswith(">"):
        i += 1
    if i < len(lines) and lines[i] == "":
        i += 1
    return "\n".join(lines[i:])


def parse_permission(text: str, truncated: bool) -> str | Refusal:
    t = text.strip().lower()
    if t in ("y", "yes", "allow"):
        return Refusal(TOO_LONG) if truncated else dumps({"behavior": "allow"})
    if t in ("n", "no", "deny"):
        return dumps({"behavior": "deny"})
    return Refusal(REPLY_Y_OR_N)


def _questions(tool_input: Obj) -> list[Obj] | None:
    raw = get_list(tool_input, "questions")
    if not raw:
        return None
    questions: list[Obj] = []
    for q in raw:
        if not isinstance(q, dict) or not _nonempty(q, "question"):
            return None
        questions.append(q)
    return questions


def _answer(index: int, question: Obj, raw: str) -> str | Refusal:
    options = _options(question)
    if not options or not _NUMBERS.fullmatch(raw):
        return raw
    numbers: list[int] = []
    for part in raw.split(","):
        part = part.strip()
        if len(part) > 3:
            return Refusal(f"Question {index}: there is no option {part}.")
        n = int(part)
        if n not in numbers:
            numbers.append(n)
    for k in numbers:
        if not 1 <= k <= len(options):
            return Refusal(f"Question {index}: there is no option {k}.")
    if len(numbers) > 1 and question.get("multiSelect") is not True:
        return Refusal(f"Question {index}: pick one option.")
    return ", ".join(get_str(options[k - 1], "label") or "" for k in numbers)


def parse_question(tool_input: Obj, text: str) -> str | Refusal:
    questions = _questions(tool_input)
    if questions is None:
        return Refusal(CANNOT_ANSWER)
    if len(questions) == 1:
        raw = [text.strip()]
        if not raw[0]:
            return Refusal(EMPTY_ANSWER)
    else:
        raw = [line.strip() for line in text.splitlines() if line.strip()]
        if len(raw) != len(questions):
            return Refusal(f"Reply with {len(questions)} lines, one per question.")
    answers: Obj = {}
    for i, (question, answer) in enumerate(zip(questions, raw, strict=True), 1):
        result = _answer(i, question, answer)
        if isinstance(result, Refusal):
            return result
        answers[get_str(question, "question") or ""] = result
    return dumps({"behavior": "allow", "answers": answers})
