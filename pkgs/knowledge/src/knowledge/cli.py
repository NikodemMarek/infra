"""Knowledge curator: collects what agents learn and keeps a curated store in the memory repo."""

import argparse
import datetime
import logging
import os
import sys
from pathlib import Path

from . import ask, config, curate, data, decisions, emit, hooks, importer, index, llm, locks, records, retention, route
from .config import UsageError
from .data import Json, Obj, ParseError, dumps
from .entry import KINDS
from .gitops import Git, GitError
from .inbox import Inbox
from .manual import Manual
from .ops import MODES
from .records import Origin
from .scopes import DEFAULT_SCOPES_TOML, SCOPES_FILE
from .store import Store, atomic_write

log = logging.getLogger("knowledge")

GITIGNORE = ".gitignore"
PRIVATE_IGNORE = "private/"
FILE_MODE = 0o644


class Args(argparse.Namespace):
    command: str
    verbose: bool
    title: str
    kind: str | None
    scope: str | None
    evidence: str | None
    file: str | None
    user: bool
    paths: list[str]
    trust: str
    sid: str
    mode: str
    weekly: bool
    if_idle: bool
    dry_run: bool
    print_prompt: bool
    path: str
    reason: str | None
    taken: bool
    hook_name: str
    question: list[str]
    task: list[str]


def _parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog="knowledge", description=__doc__)
    p.add_argument("-v", "--verbose", action="store_true")
    sub = p.add_subparsers(dest="command", required=True)
    sub.add_parser("paths", help="print every path, the router URL and the claude binary as JSON")
    sub.add_parser("init", help="prepare the memory repo: SCOPES.toml, .gitignore and KNOWLEDGE.md, committed")
    sub.add_parser("index", help="regenerate KNOWLEDGE.md and commit it if it changed")
    sub.add_parser("lint", help="check the store, SCOPES.toml and the budgets; exit 1 if anything is wrong")
    submit = sub.add_parser("submit", help="send what you learned to the curator through the event router; the body is read from stdin")
    submit.add_argument("--title", required=True, help="one line")
    submit.add_argument("--kind", choices=KINDS)
    submit.add_argument("--scope", help="a hint, e.g. topic/jj; the curator decides")
    submit.add_argument("--evidence", help="where it was observed")
    submit.add_argument("--file", help="read the body from this file instead of stdin (not inside Claude Code: pipe the body there)")
    submit.add_argument("--user", action="store_true", help="a submission of the user, stored directly with user trust; needs a terminal")
    asker = sub.add_parser("ask", help="ask the memory (curated knowledge and project memory) a question; prints the answer")
    asker.add_argument("question", nargs="*", metavar="QUESTION", help="the question; read from stdin if omitted")
    router = sub.add_parser(
        "route", help="pick the repo a task belongs in among the repo directories on stdin (one per line); prints one JSON line"
    )
    router.add_argument("task", nargs="*", metavar="TASK", help="the task; put -- before one that starts with a dash")
    sub.add_parser("receive", help="store a knowledge.submit event read from stdin (run by the event router)")
    imp = sub.add_parser("import", help="submit notes (e.g. Claude Code auto-memory files) to the curator, stored directly")
    imp.add_argument("paths", nargs="+", metavar="PATH", help="markdown files, or directories of them (MEMORY.md is skipped)")
    imp.add_argument(
        "--trust",
        choices=("user", "agent"),
        default="user",
        help="user (the default) asks for confirmation and needs a terminal; agent needs neither",
    )
    status = sub.add_parser("status", help="what became of a submission: applied, rejected, failed, pending, bad or unknown")
    status.add_argument("sid", metavar="ID", help="the id `knowledge submit` printed")
    cur = sub.add_parser("curate", help="run the curator over the pending submissions: one model call, then apply its decisions")
    cur.add_argument("--mode", choices=MODES, default="intake", help="weekly also maintains the store (stale entries, budgets, skill ideas)")
    cur.add_argument("--weekly", action="store_true", help="same as --mode weekly")
    cur.add_argument("--if-idle", action="store_true", help="do nothing, and exit 0, if another run is going (the default is to wait for it)")
    cur.add_argument("--dry-run", action="store_true", help="call the model and print what it decided; change nothing")
    cur.add_argument("--print-prompt", action="store_true", help="print the system prompt and the message, without calling the model")
    undo = sub.add_parser("undo", help="revert every commit of a submission (one new commit; exit 1 on a conflict)")
    undo.add_argument("sid", metavar="ID", help="the id `knowledge submit` printed")
    restore = sub.add_parser("restore", help="bring back an entry from the commit that deleted it")
    restore.add_argument("path", metavar="PATH", help="the entry, e.g. topic/jj/absorb.md")
    drop = sub.add_parser(
        "drop", help="delete an entry of any trust or a skill idea; the curator is told not to take it back"
    )
    drop.add_argument("path", metavar="PATH", help="the entry, e.g. topic/jj/absorb.md, or skill-ideas/<name>.md")
    drop.add_argument("--reason", help="why; kept in the commit message")
    drop.add_argument(
        "--taken",
        action="store_true",
        help="a skill idea you adopted as a skill: delete it without telling the curator to leave it alone (plain drop rejects it)",
    )
    hook = sub.add_parser("hook", help="a Claude Code hook: reads the hook input on stdin, prints its JSON output, always exits 0")
    hook.add_argument("hook_name", choices=hooks.HOOKS, metavar="HOOK", help="session-start (curated knowledge for the directory) or subagent-stop (submit the knowledge blocks of the last message)")
    return p


def _paths() -> Obj:
    return {
        "memory_dir": str(config.memory_dir()),
        "claude_config_dir": str(config.claude_config_dir()),
        "state_dir": str(config.state_dir()),
        "inbox": str(config.inbox_dir()),
        "runtime_dir": str(config.runtime_dir()),
        "skills_dir": str(config.skills_dir()),
        "router_url": config.router_url(),
        "claude": config.claude_bin(),
        "memory_lock": str(locks.memory_lock()),
        "curator_lock": str(locks.curator_lock()),
    }


def _memory() -> Path:
    memory = config.memory_dir()
    if not memory.is_dir():
        raise UsageError(f"{memory} is not a directory")
    return memory


def _git(memory: Path) -> Git:
    """The memory repo, which must be a git repository of its own."""
    git = Git(memory, locks.memory_lock())
    try:
        git.ensure_repo()
    except GitError as e:
        raise UsageError(f"{memory} is not a git repository: {e}") from e
    return git


def _ensure_private_ignored(memory: Path) -> None:
    """`private/` (credentials the curator keeps for the user) must never be committed."""
    path = memory / GITIGNORE
    try:
        text = path.read_text(encoding="utf-8")
    except FileNotFoundError:
        text = ""
    if any(line.strip() in (PRIVATE_IGNORE, "/" + PRIVATE_IGNORE) for line in text.splitlines()):
        return
    if text and not text.endswith("\n"):
        text += "\n"
    atomic_write(path, f"{text}{PRIVATE_IGNORE}\n", FILE_MODE)


def _stale_flags(memory: Path) -> dict[str, list[str]]:
    """The markers a curator run writes into the index, so that this does not undo them."""
    store = Store(memory)
    today = datetime.datetime.now(datetime.UTC).date()
    return retention.stale_flags(store.entries(), today, retention.done_slugs(store.projects_index()))


def _init() -> Obj:
    memory = _memory()
    git = _git(memory)
    if not (memory / SCOPES_FILE).exists():
        atomic_write(memory / SCOPES_FILE, DEFAULT_SCOPES_TOML, FILE_MODE)
    _ensure_private_ignored(memory)
    entries = index.regenerate(memory, _stale_flags(memory))
    commit = git.commit([SCOPES_FILE, GITIGNORE, index.INDEX_FILE], "knowledge: init")
    return {"commit": commit, "entries": entries}


def _index() -> Obj:
    memory = _memory()
    git = _git(memory)
    entries = index.regenerate(memory, _stale_flags(memory))
    commit = git.commit([index.INDEX_FILE], "knowledge: update index")
    return {"commit": commit, "entries": entries}


def _lint() -> tuple[Obj, int]:
    store = Store(_memory())
    problems = index.lint(store)
    for problem in problems:
        log.error("%s", problem)
    listed: list[Json] = [p for p in problems]
    return {"ok": not problems, "problems": listed}, 1 if problems else 0


def _read_body(file: str | None, terminal: bool = False) -> str:
    """From the file, else stdin; `terminal`: a terminal is read too (up to end of input, Ctrl-D)."""
    if file is None:
        if terminal and data.stdin_is_tty():
            log.info("type the body, then Ctrl-D")
        body = data.read_stdin(terminal)
    else:
        try:
            body = Path(file).read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError) as e:
            raise UsageError(f"cannot read {file}: {e}") from e
    if not body.strip():
        raise UsageError("empty body: pipe it on stdin or pass --file")
    return body


def _require_tty(what: str, hint: str) -> None:
    """Trust `user` is only for a person at a terminal."""
    if not data.stdin_is_tty():
        raise UsageError(f"{what} needs a terminal: {hint}")


def _require_human(what: str, hint: str) -> None:
    """Trust `user` is not for an agent: refuse inside an event-launched run, an event workspace or Claude Code.

    This raises the bar rather than sealing it: a process of the same uid can still write the inbox directly, or
    unset these variables.
    """
    if config.origin() is not None:
        raise UsageError(f"{what} is not for agents: KNOWLEDGE_ORIGIN is set (an event launched this run); {hint}")
    if records.in_agent_workspace(os.getcwd()):
        raise UsageError(f"{what} is not for agents: this is the workspace of an event-launched agent; {hint}")
    if config.in_claude_code():
        raise UsageError(f"{what} is not for agents: CLAUDECODE is set (this runs inside Claude Code); {hint}")


def _submit(args: Args) -> str:
    """Post the submission to the router, or with --user put it in the inbox; its id."""
    if args.file is not None and config.in_claude_code():  # an agent could send any file it names without a Read prompt
        raise UsageError("submit --file is not for agents: CLAUDECODE is set (this runs inside Claude Code); pipe the body on stdin")
    if args.user:
        _require_human("submit --user", "submit without --user")
        _require_tty("submit --user", "run it in one, or submit without --user")
        origin = Origin(via="user", agent=None, cwd=os.getcwd(), event=None, session=None)
    else:
        origin = Origin(via="cli", agent=config.agent_name(), cwd=os.getcwd(), event=config.origin(), session=None)
    rec = records.from_event(
        {
            "title": args.title,
            "body": _read_body(args.file, terminal=args.user),
            "kind": args.kind,
            "scope": args.scope,
            "evidence": args.evidence,
            "origin": records.origin_obj(origin),
        },
        via_router=False,
    )
    if args.scope is not None and rec.scope is None:
        log.warning("ignoring scope %r: not a valid scope", args.scope)
    if (secret := emit.hard_secret(rec)) is not None:
        raise UsageError(f"refusing to submit: looks like a secret ({secret})")
    if args.user:
        Inbox().put(rec)  # no router: the trust is that of the terminal
    else:
        emit.post(emit.build_event(rec), config.router_url())
    return rec.id


def _import(args: Args) -> tuple[Obj, int]:
    """Put the notes in the inbox as submissions, with the trust asked for (`user` after a confirmation)."""
    if args.trust == "user":
        _require_human("import with --trust user", "pass --trust agent")
        _require_tty("import with --trust user", "run it in one, or pass --trust agent")
    origin = Origin(via="import", agent=None, cwd=os.getcwd(), event=config.origin(), session=None)
    files = importer.collect(args.paths)
    if not files:
        raise UsageError("no markdown files to import")
    if args.trust == "user":
        answer = data.ask(f"Import {len(files)} file(s) as knowledge you vouch for (trust user)? [y/N] ")
        if answer.strip().lower() not in ("y", "yes"):
            log.error("aborted: nothing imported")
            return {"imported": 0, "skipped": 0, "ids": []}, 1
    return importer.result_obj(importer.import_files(files, args.trust, Inbox(), origin)), 0


def _receive() -> Obj:
    """Put the event from the router in the inbox, unless it looks like a secret. The timers start the curator."""
    event = data.as_obj(data.loads(data.read_stdin()), "event")
    type_ = data.get_str(event, "type")
    if type_ != emit.EVENT_TYPE:
        raise UsageError(f"expected an event of type {emit.EVENT_TYPE}, got {type_!r}")
    rec = records.from_event(event, via_router=True)
    if (secret := emit.hard_secret(rec)) is not None:
        log.warning("dropping submission %s: looks like a secret (%s)", rec.id, secret)
        return {"id": rec.id, "stored": False}
    inbox = Inbox()
    if (state := inbox.state(rec.id)) is not None:  # the first post wins: a replay must not reset the attempt count of a
        log.info("submission %s is queued already (%s)", rec.id, state)  # pending record, or bring back a failed or bad one
    else:
        inbox.put(rec)
    return {"id": rec.id, "stored": True}


def _status(args: Args) -> Obj:
    return decisions.status(args.sid, decisions.Decisions(), Inbox().state(args.sid))


def _ask(args: Args) -> str:
    """The librarian's answer to the question from the arguments, else from stdin."""
    question = " ".join(args.question).strip() or data.read_stdin().strip()
    if not question:
        raise UsageError("pass the question as arguments or on stdin")
    _memory()
    return ask.answer(question, Path(os.getcwd()))


def _route(args: Args) -> str:
    """The repo the task belongs in among the directories on stdin, as one JSON line: {"repo": name or null, "reason": ...}."""
    task = " ".join(args.task)
    if not task.strip():
        raise UsageError("pass the task as arguments")
    candidates = [Path(line.strip()) for line in data.read_stdin().splitlines() if line.strip()]
    if not candidates:
        raise UsageError("pass the repo directories on stdin, one per line")
    _memory()
    picked = route.route(task, candidates)
    out: Obj = {"repo": picked.repo, "reason": picked.reason}
    return dumps(out)


def _curate(args: Args) -> tuple[Obj | str, int]:
    """Run the curator; the summary, or the prompt/preview a flag asks for. Exit 1 if the model call failed."""
    weekly = args.weekly or args.mode == "weekly"
    if args.dry_run or args.print_prompt:  # these change nothing, so there is no next run to go on with
        summary = curate.run(weekly=weekly, if_idle=args.if_idle, dry_run=args.dry_run, print_prompt=args.print_prompt)
    else:
        summary = curate.run_pending(weekly=weekly, if_idle=args.if_idle)
    code = 1 if summary.skipped in curate.FAILURES else 0
    return (curate.summary_obj(summary) if summary.text is None else summary.text), code


def _hook(name: str) -> None:
    """Print what the hook has to say, if anything. Whatever happens, the exit code is 0: a hook must not block the session."""
    out = hooks.run(name)
    if out is None:
        return
    try:
        print(dumps(out), flush=True)
    except (OSError, ValueError) as e:  # a closed pipe; text that cannot be written out
        log.warning("hook %s: cannot print its output: %s", name, e)


def main() -> None:
    args = _parser().parse_args(namespace=Args())
    logging.basicConfig(
        level=logging.DEBUG if args.verbose else logging.INFO,
        format="%(levelname)s %(message)s",
        stream=sys.stderr,
    )
    if args.command == "hook":
        _hook(args.hook_name)
        return
    code = 0
    try:
        result: Obj | str
        if args.command == "paths":
            result = _paths()
        elif args.command == "init":
            result = _init()
        elif args.command == "index":
            result = _index()
        elif args.command == "lint":
            result, code = _lint()
        elif args.command == "submit":
            result = _submit(args)  # the id alone, for scripts and agents
        elif args.command == "ask":
            result = _ask(args)
        elif args.command == "route":
            result = _route(args)
        elif args.command == "receive":
            result = _receive()
        elif args.command == "import":
            result, code = _import(args)
        elif args.command == "status":
            result = _status(args)
        elif args.command == "curate":
            result, code = _curate(args)
        elif args.command == "undo":
            result = Manual(_git(_memory())).undo(args.sid)
        elif args.command == "restore":
            result = Manual(_git(_memory())).restore(args.path)
        elif args.command == "drop":
            result = Manual(_git(_memory())).drop(args.path, args.reason, args.taken)
        else:  # argparse accepts only the commands above
            raise UsageError(f"unknown command {args.command}")
        print(result if isinstance(result, str) else dumps(result), flush=True)
    except (ParseError, UsageError) as e:
        log.error("%s", e)
        sys.exit(2)
    except emit.EmitError as e:
        if e.permanent:
            log.error("knowledge: router rejected the submission (%s); knowledge NOT stored", e)
            sys.exit(2)
        log.error("knowledge: router unreachable (%s); knowledge NOT stored", e)
        sys.exit(1)
    except (OSError, ValueError, GitError, locks.LockTimeout, llm.LlmError) as e:  # ValueError: text that cannot be written out
        log.error("%s", e)
        sys.exit(1)
    if code:
        sys.exit(code)
