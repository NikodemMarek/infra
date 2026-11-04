import math
import threading
from collections.abc import Callable, Mapping, Sequence

from .mask import mask
from .model import (
    Action,
    ActionInfo,
    Answer,
    Config,
    Denied,
    Failed,
    Listed,
    Pending,
    Ran,
    Response,
    RunReq,
    RunResult,
    State,
)

type Clock = Callable[[], float]  # seconds; prod: time.clock_gettime(time.CLOCK_BOOTTIME)
type Ask = Callable[[Pending], Answer]  # blocking; may call Broker.unlock() inside
type Run = Callable[[Sequence[str], Mapping[str, str], int], RunResult]  # argv, secret env, timeout

GRANT_SECONDS = 900
OUTPUT_CAP = 4 * 1024 * 1024


class Broker:
    def __init__(self, config: Config, clock: Clock, ask: Ask, run: Run) -> None:
        self._config = config
        self._clock = clock
        self._ask = ask
        self._run = run
        # Guards the state below; never held while calling ask or run.
        self._lock = threading.Lock()
        self._unlocked = False
        self._values: dict[str, str] = {}
        self._errors: dict[str, str] = {}
        self._grants: dict[str, float] = {}  # action -> expiry
        self._used: set[str] = set()  # actions run since start
        self._last_use = 0.0

    @property
    def locked(self) -> bool:
        self.tick()
        return not self._unlocked

    def referenced_items(self) -> list[str]:
        """Sorted unique item names over all actions."""
        self.tick()
        return sorted({i for a in self._config.actions.values() for i in a.items()})

    def unlock(self, values: Mapping[str, str], errors: Mapping[str, str]) -> None:
        """Item name -> value / reason; sets last_use = now."""
        self.tick()
        with self._lock:
            self._unlocked = True
            self._values = dict(values)
            self._errors = dict(errors)
            self._last_use = self._clock()

    def lock(self) -> None:
        """Clears values, errors and grants."""
        self.tick()
        with self._lock:
            self._clear()

    def tick(self) -> None:
        """Lock if unlocked and now >= last_use + lock_after * 60."""
        with self._lock:
            if self._unlocked and self._clock() >= self._last_use + self._config.lock_after * 60:
                self._clear()

    def list_actions(self) -> list[ActionInfo]:
        """Sorted by name."""
        self.tick()
        infos: list[ActionInfo] = []
        with self._lock:
            now = self._clock()
            for name, a in sorted(self._config.actions.items()):
                state: State = "ready"
                detail = ""
                reason = self._item_error(a)
                if not self._unlocked:
                    state = "locked"
                elif reason:
                    state, detail = "unavailable", reason
                elif self._grants.get(name, 0.0) > now:
                    state, detail = "granted", f"{math.ceil((self._grants[name] - now) / 60)} min left"
                infos.append(
                    ActionInfo(
                        name=name,
                        description=a.description,
                        args=tuple(p.pattern for p in a.args),
                        items=a.items(),
                        state=state,
                        detail=detail,
                    )
                )
        return infos

    def handle_run(self, req: RunReq) -> Response:
        self.tick()
        action = self._config.actions.get(req.action)
        if action is None:
            return Denied("unknown action")
        if len(req.args) != len(action.args) or not all(
            p.fullmatch(arg) for p, arg in zip(action.args, req.args)
        ):
            return Denied("arguments do not match")

        with self._lock:
            locked = not self._unlocked
            reason = None if locked else self._item_error(action)
            first_use = action.name not in self._used
            granted = not locked and not first_use and self._grants.get(action.name, 0.0) > self._clock()
        if reason:
            return Denied(reason)

        grant = False
        if not granted:
            answer = self._ask(Pending(action, req.args, req.cwd, req.session, first_use, locked))
            if answer == "deny":
                return Denied("denied on the console")
            if answer == "timeout":
                return Denied("approval timed out")
            grant = answer == "grant"

        with self._lock:
            if not self._unlocked:
                return Denied("vault locked")
            reason = self._item_error(action)
            if reason:
                return Denied(reason)
            if grant:
                self._grants[action.name] = self._clock() + GRANT_SECONDS
            env = {name: self._values[item] for name, item in action.secrets.items()}
            secrets = [self._values[item].encode() for item in action.items()]

        result = self._run([*action.command, *req.args], env, action.timeout)
        with self._lock:
            self._used.add(action.name)
            self._last_use = self._clock()

        if result.timed_out:
            return Failed(f"action timed out after {action.timeout} s")
        out, err = result.stdout, result.stderr
        if result.overflow:
            # A secret cut off at the end of the kept bytes would survive masking as a prefix.
            drop = max((len(s) for s in secrets), default=1) - 1
            if drop > 0:
                out, err = out[:-drop], err[:-drop]
        stdout = mask(out, secrets)
        stderr = mask(err, secrets)
        cut = len(stdout) > OUTPUT_CAP or len(stderr) > OUTPUT_CAP
        return Ran(
            exit=result.exit,
            stdout=stdout[:OUTPUT_CAP],
            stderr=stderr[:OUTPUT_CAP],
            truncated=result.overflow or cut,
        )

    def _clear(self) -> None:
        self._unlocked = False
        self._values = {}
        self._errors = {}
        self._grants = {}

    def _item_error(self, action: Action) -> str | None:
        """The reason the first unusable item of `action` is unusable; needs the lock."""
        for item in action.items():
            if item in self._errors:
                return self._errors[item]
            if item not in self._values:
                return f"item missing: {item}"
        return None
