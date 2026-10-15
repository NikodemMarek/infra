"""Knowledge curator: collects what agents learn and keeps a curated store in the memory repo."""

import argparse
import logging
import os
import sys
from pathlib import Path

from . import config, curate, data, hooks, llm, locks, notes, route, skills
from .config import UsageError
from .data import Obj, ParseError, dumps

log = logging.getLogger("knowledge")


class Args(argparse.Namespace):
    command: str
    verbose: bool
    hook_name: str
    task: list[str]


def _parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog="knowledge", description=__doc__)
    p.add_argument("-v", "--verbose", action="store_true")
    sub = p.add_subparsers(dest="command", required=True)
    sub.add_parser("submit", help="queue a note for the curator: the note is read from stdin, its path is printed")
    sub.add_parser("curate", help="merge the queued notes into the scope files: one model call, checks, a commit (run by a timer)")
    sub.add_parser("skills", help="draft skill ideas from the memory into skill-ideas/ and print their paths (on demand)")
    router = sub.add_parser(
        "route", help="pick the repo a task belongs in among the repo directories on stdin (one per line); prints one JSON line"
    )
    router.add_argument("task", nargs="*", metavar="TASK", help="the task; put -- before one that starts with a dash")
    hook = sub.add_parser("hook", help="a Claude Code hook: reads the hook input on stdin, prints its JSON output, always exits 0")
    hook.add_argument(
        "hook_name",
        choices=hooks.HOOKS,
        metavar="HOOK",
        help="session-start (the scope files for the directory) or subagent-stop (queue the knowledge blocks of the last message as notes)",
    )
    return p


def _memory() -> Path:
    memory = config.memory_dir()
    if not memory.is_dir():
        raise UsageError(f"{memory} is not a directory")
    return memory


def _submit() -> str:
    """Queue the note on stdin; the path of its file."""
    text = data.read_stdin()
    if not text.strip():
        raise UsageError("pass the note on stdin")
    try:
        return str(notes.write(text, notes.origin_now(), Path(os.getcwd())))
    except ValueError as e:
        raise UsageError(f"refusing to submit: {e}") from e


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
        result: str | None = None
        if args.command == "submit":
            result = _submit()  # the path alone, for scripts and agents
        elif args.command == "route":
            result = _route(args)
        elif args.command == "curate":
            code = curate.run()
        elif args.command == "skills":
            code = skills.run()
        else:  # argparse accepts only the commands above
            raise UsageError(f"unknown command {args.command}")
        if result is not None:
            print(result, flush=True)
    except (ParseError, UsageError) as e:
        log.error("%s", e)
        sys.exit(2)
    except (OSError, ValueError, locks.LockTimeout, llm.LlmError) as e:  # ValueError: text that cannot be written out
        log.error("%s", e)
        sys.exit(1)
    if code:
        sys.exit(code)
