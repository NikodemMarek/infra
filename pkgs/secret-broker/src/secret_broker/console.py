import os
import secrets
import select
import sys
import termios
import threading
import time
from collections.abc import Callable, Iterator, Mapping, Sequence
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
from typing import Literal, TextIO, cast

from . import bw as bitwarden
from .broker import Broker
from .model import Action, Answer, Config, Denied, ExportReq, Pending
from .runner import SWEEP_FAILED, own_cgroup_procs, sweep
from .text import clean, clean_block

TAG_ALPHABET = "abcdefghjkmnpqrstuvwxyz"
PROMPT_TIMEOUT = 300
POLL = 0.2
WITHDRAWN_PAUSE = 1
# keep every screen short enough to show the phrase and tag above the prompt
MAX_LISTED = 10
MAX_TITLES = 30
SCRIPT_LINES = 20
SCRIPT_CHARS = 1500
DENIED = Denied("denied on the console")

type _Attrs = list[int | list[bytes | int]]


def parse_answer(text: str) -> Literal["once", "grant", "deny", "retry"]:
    """`text.strip().lower()`: y once, g grant, n deny, anything else retry."""
    match text.strip().lower():
        case "y":
            return "once"
        case "g":
            return "grant"
        case "n":
            return "deny"
        case _:
            return "retry"


def new_tag() -> str:
    return "".join(secrets.choice(TAG_ALPHABET) for _ in range(4))


def load_phrase(state_dir: Path, tty_in: TextIO, tty_out: TextIO) -> str:
    """Read `state_dir/phrase`; on the first start ask for it twice and write it 0600."""
    path = state_dir / "phrase"
    try:
        return path.read_text(encoding="utf-8").rstrip("\n")
    except FileNotFoundError:
        pass

    def ask(prompt: str) -> str:
        tty_out.write(prompt)
        tty_out.flush()
        line = tty_in.readline()
        if not line:
            raise SystemExit("secret-broker: console closed while asking for the phrase")
        return line.rstrip("\n")

    while True:
        phrase = ask("Choose a greeting phrase, shown above every password prompt: ")
        if phrase and phrase == ask("Again: "):
            break
        tty_out.write("Empty or different, try again.\n")
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8") as f:
        f.write(phrase + "\n")
    return phrase


@dataclass(eq=False)
class _Item:
    """A queued request: a run approval (`pending`) or an export (`export`)."""

    cancel: threading.Event
    deadline: float
    pending: Pending | None
    export: ExportReq | None
    tag: str
    titles: Mapping[str, Sequence[str]]
    done: bool = False
    answer: Answer = "deny"
    password: str | Denied = DENIED


def disable_signal_keys(fd: int) -> None:
    """Clear ISIG so Ctrl+C, Ctrl+\\ and Ctrl+Z on the console cannot signal the broker."""
    attrs = cast(_Attrs, termios.tcgetattr(fd))
    attrs[3] = cast(int, attrs[3]) & ~termios.ISIG
    termios.tcsetattr(fd, termios.TCSANOW, attrs)


def _script(path: str) -> str | None:
    """The contents of `path` when it is a UTF-8 text file; a note when it is too large to show."""
    try:
        with open(path, "rb") as f:
            raw = f.read(65537)
        if b"\0" in raw:
            return None
        if len(raw) > 65536:
            return "(script not shown: too large)"
        s = raw.decode()
    except (OSError, UnicodeDecodeError):
        return None
    return clean_block(s, SCRIPT_LINES, SCRIPT_CHARS)


def _more[T](items: Sequence[T], limit: int = MAX_LISTED) -> tuple[Sequence[T], list[str]]:
    """The first `limit` items and a "… N more" line when there are more."""
    return items[:limit], [f"  … {len(items) - limit} more"] if len(items) > limit else []


def _describe(a: Action) -> list[str]:
    lines = ["first use since start, full action:", f"  command: {' '.join(clean(c) for c in a.command)}"]
    pats, more = _more(list(a.args))
    lines += [f"  arg {i} pattern: {clean(p.pattern)}" for i, p in enumerate(pats, 1)] + more
    envs, more = _more(sorted(a.secrets.items()))
    lines += [f"  {clean(env)} <- item {clean(item)}" for env, item in envs] + more
    script = _script(a.command[0])
    if script is not None:
        lines += [f"  contents of {clean(a.command[0])}:", script]
    return lines


def _title_lines(titles: Mapping[str, Sequence[str]]) -> list[str]:
    """Titles per profile, at most MAX_TITLES in all, then "… N more"."""
    lines: list[str] = []
    shown = 0
    for profile, ts in titles.items():
        if shown >= MAX_TITLES:
            break
        part = ts[: MAX_TITLES - shown]
        lines += [f"profile {clean(profile)}:", *[f"  {clean(t)}" for t in part]]
        shown += len(part)
    total = sum(len(ts) for ts in titles.values())
    return lines + [f"… {total - shown} more"] if total > shown else lines


class Console:
    """The tty: serial queue of approvals and unlocks, idle commands. Verified live, not unit-tested."""

    def __init__(
        self,
        broker: Broker,
        config: Config,
        bw: str,
        env: Mapping[str, str],
        lock: threading.Lock,
        phrase: str,
        tty_in: TextIO,
        tty_out: TextIO,
    ) -> None:
        """`env` is the base env for bw; `lock` is serve's global lock (taken around unlocks)."""
        self._broker = broker
        self._config = config
        self._bw = bw
        self._env = env
        self._lock = lock
        self._phrase = phrase
        self._in = tty_in
        self._out = tty_out
        self._out_lock = threading.Lock()
        # guards the queue and every item's result
        self._cv = threading.Condition()
        self._queue: list[_Item] = []

    def ask(self, pending: Pending, cancel: threading.Event) -> Answer:
        """Queue a run approval and block for the answer; may unlock the broker; `cancel` withdraws it."""
        item = _Item(cancel, time.monotonic() + PROMPT_TIMEOUT, pending, None, "", {})
        self._submit(item)
        return item.answer

    def approve_export(
        self,
        req: ExportReq,
        tag: str,
        titles: Mapping[str, Sequence[str]],
        cancel: threading.Event,
    ) -> str | Denied:
        """Queue an export (titles per profile from `resolved_titles`); the master password, or why not."""
        item = _Item(cancel, time.monotonic() + PROMPT_TIMEOUT, None, req, tag, titles)
        self._submit(item)
        return item.password

    def note(self, text: str) -> None:
        """Print a line on the console (e.g. the sync note)."""
        self._say(clean(text))

    def loop(self) -> None:
        """Serve the queue and the idle keys (Ctrl+L redraw, `l` lock); never returns."""
        drawn: bool | None = None  # the locked state the idle screen shows; None = no idle screen
        while True:
            self._broker.tick()
            item = self._next()
            try:
                if item is None:
                    locked = self._broker.locked
                    if drawn != locked:
                        self._draw_idle(locked)
                        drawn = locked
                    if self._idle_key(locked):
                        self._say("\x1b[2J\x1b[H")
                        drawn = None
                    continue
                drawn = None
                if item.pending is not None:
                    self._serve_run(item, item.pending)
                elif item.export is not None:
                    self._serve_export(item, item.export)
            except Exception as e:  # the console must outlive any one request
                print(f"secret-broker: console error: {e!r}", file=sys.stderr)
                if item is not None:
                    self._finish(item)
                time.sleep(1)

    def _submit(self, item: _Item) -> None:
        with self._cv:
            self._queue.append(item)
            self._cv.notify_all()
            while not item.done:
                if time.monotonic() >= item.deadline:
                    item.answer, item.password = "timeout", Denied("approval timed out")
                    item.done = True
                elif item.cancel.is_set():
                    item.done = True
                else:
                    self._cv.wait(POLL)
            self._queue.remove(item)

    def _finish(self, item: _Item, answer: Answer = "deny", password: str | Denied = DENIED) -> None:
        with self._cv:
            if not item.done:
                item.answer, item.password, item.done = answer, password, True
                self._cv.notify_all()

    def _next(self) -> _Item | None:
        with self._cv:
            return next((i for i in self._queue if not i.done), None)

    def _waiting(self) -> int:
        with self._cv:
            return sum(1 for i in self._queue if not i.done)

    def _exports_waiting(self) -> int:
        with self._cv:
            return sum(1 for i in self._queue if not i.done and i.export is not None)

    def _say(self, *lines: str) -> None:
        with self._out_lock:
            self._out.write("\n".join(lines) + "\n")
            self._out.flush()

    def _header(self) -> list[str]:
        return ["-" * 60, f"phrase: {clean(self._phrase)}"]

    def _read(
        self,
        prompt: str,
        stop: Callable[[], bool],
        echo: bool = True,
        ended: Callable[[], str] | None = None,
    ) -> str | None:
        """One line from the tty without its newline; None once `stop()`. Pending input is dropped first.

        With `ended`, a stop means the request is gone: say what `ended()` returns, and keep echo
        off while typed-ahead keys are flushed so they cannot show up on the next screen.
        """
        fd = self._in.fileno()
        saved = cast(_Attrs, termios.tcgetattr(fd))
        quiet = list(saved)
        quiet[3] = cast(int, saved[3]) & ~termios.ECHO
        try:
            if not echo:
                termios.tcsetattr(fd, termios.TCSADRAIN, quiet)
            termios.tcflush(fd, termios.TCIFLUSH)
            with self._out_lock:
                self._out.write(prompt)
                self._out.flush()
            while not stop():
                if select.select([fd], [], [], POLL)[0]:
                    raw = os.read(fd, 4096)
                    if raw:
                        return raw.decode(errors="replace").rstrip("\n")
                    time.sleep(1)  # the tty hung up
            if ended is not None:
                termios.tcsetattr(fd, termios.TCSADRAIN, quiet)
                self._say("", ended())
                time.sleep(WITHDRAWN_PAUSE)
                termios.tcflush(fd, termios.TCIFLUSH)
            return None
        finally:
            if not echo or ended is not None:
                termios.tcsetattr(fd, termios.TCSADRAIN, saved)
            if not echo:
                self._say("")

    def _prompt(self, item: _Item, prompt: str, echo: bool = True) -> str | None:
        """`_read` for a queued item, with the phrase (and the export tag) on the lines directly above."""
        lines = [f"phrase: {clean(self._phrase)}"]
        if item.export is not None:
            lines.append(f"tag: {item.tag}")
        self._say(*lines)
        return self._read(
            prompt,
            lambda: item.done,
            echo,
            ended=lambda: "request timed out" if item.answer == "timeout" else "request withdrawn",
        )

    def _draw_idle(self, locked: bool) -> None:
        state = "LOCKED" if locked else "unlocked"
        self._say(*self._header(), f"idle, {state}. l = lock now, Ctrl+L = redraw (finish with Enter)")

    def _idle_stop(self, locked: bool) -> bool:
        """Poll of the idle read: runs the auto-lock; True on a request or when the lock state changed."""
        self._broker.tick()
        return self._broker.locked != locked or self._next() is not None

    def _idle_key(self, locked: bool) -> bool:
        """Handle one idle line; True when it asked for a redraw."""
        line = self._read("", lambda: self._idle_stop(locked))
        if line is None:
            return False
        if line.strip().lower() == "l":
            self._broker.lock()
            self._say("locked, grants cleared")
        return "\x0c" in line

    def _serve_run(self, item: _Item, p: Pending) -> None:
        if self._broker.locked:
            self._serve_locked(item)
            return
        a = p.action
        lines = [
            *self._header(),
            f"RUN request, {self._waiting() - 1} more waiting",
            f"action: {clean(a.name)}: {clean(a.description)}",
            *[f"  arg {i}: {clean(arg)}" for i, arg in enumerate(p.args, 1)],
            f"claimed cwd: {clean(p.cwd)}",
            f"claimed session: {clean(p.session)}",
        ]
        if p.first_use:
            lines += _describe(a)
        self._say(*lines)
        while True:
            line = self._prompt(item, f"run {clean(a.name)}? [y] once  [g] 15 min  [n] deny: ")
            if line is None:
                return
            answer = parse_answer(line)
            if answer != "retry":
                self._finish(item, answer)
                return

    def _serve_locked(self, item: _Item) -> None:
        self._say(*self._header(), f"LOCKED, {self._waiting()} waiting, Enter to unlock (n = deny this request)")
        line = self._prompt(item, "> ")
        if line is None:
            return
        match line.strip().lower():
            case "":
                self._unlock(item)
            case "n":
                self._finish(item)

    def _serve_export(self, item: _Item, req: ExportReq) -> None:
        problem = self._ready()
        if problem is not None:
            self.note(problem)
            self._finish(item, password=Denied(problem))
            return
        self._say(
            *self._header(),
            f"EXPORT request, tag {item.tag}, {self._exports_waiting()} export(s) waiting (including this one)",
            f"claimed cwd: {clean(req.cwd)}",
            f"claimed session: {clean(req.session)}",
        )
        self._say(*_title_lines(item.titles))
        password = self._prompt(
            item, "Master password to approve, only if the tag matches your terminal (empty = deny): ", echo=False
        )
        if password is not None:
            self._finish(item, password=password or DENIED)

    def _ready(self) -> str | None:
        """Why bw cannot be used (None = it can): the server is pinned and the account logged in."""
        with self._lock:
            if not sweep(own_cgroup_procs(), os.getpid()):
                return SWEEP_FAILED
            try:
                st = bitwarden.status(self._bw, self._env)
                if st.get("status") == "unauthenticated":
                    # a fresh bw talks to the default server, which `check_server` would reject
                    if self._config.bw_server is not None:
                        bitwarden.config_server(self._bw, self._env, self._config.bw_server)
                    self._say("bw is not logged in: bw login --apikey")
                    fd = self._in.fileno()
                    saved = cast(_Attrs, termios.tcgetattr(fd)) if os.isatty(fd) else None
                    try:
                        bitwarden.login(self._bw, self._env, fd, self._out.fileno())
                    finally:
                        # a killed bw can leave the tty raw
                        if saved is not None:
                            termios.tcsetattr(fd, termios.TCSANOW, saved)
                            termios.tcflush(fd, termios.TCIFLUSH)
                    st = bitwarden.status(self._bw, self._env)
                bitwarden.check_server(st, self._config.bw_server)
            except bitwarden.BwError as e:
                return str(e)
        return None

    @contextmanager
    def vault_session(self, password: str) -> Iterator[str]:
        """Under the global lock: sweep, check the server, unlock bw, sync and load every referenced item into the broker.

        Yields the bw session; raises `BwError`. Notes a failed sync and every item error on the console.
        """
        with self._lock:
            if not sweep(own_cgroup_procs(), os.getpid()):
                raise bitwarden.BwError(SWEEP_FAILED)
            bitwarden.check_server(bitwarden.status(self._bw, self._env), self._config.bw_server)
            with bitwarden.unlocked(self._bw, password, self._env) as session:
                sync_note = bitwarden.sync(self._bw, session, self._env)
                if sync_note:
                    self.note(sync_note)
                values, errors = bitwarden.load(self._bw, session, self._broker.referenced_items(), self._env)
                self._broker.unlock(values, errors)
                for _, reason in sorted(errors.items()):
                    self.note(reason)
                yield session

    def _unlock(self, item: _Item) -> None:
        problem = self._ready()
        if problem is not None:
            self.note(problem)
            if problem == SWEEP_FAILED:
                self._finish(item)
            return
        password = self._prompt(item, "Master password: ", echo=False)
        if not password:
            return
        try:
            with self.vault_session(password):
                pass
        except bitwarden.BwError as e:
            self.note(str(e))
            if str(e) == SWEEP_FAILED:
                self._finish(item)
            return
        self._say("unlocked")
