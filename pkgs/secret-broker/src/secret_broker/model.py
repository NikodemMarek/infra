"""Plain data shared by the rest of the package."""

import re
from dataclasses import dataclass
from typing import Literal


@dataclass(frozen=True)
class Action:
    name: str
    description: str
    command: tuple[str, ...]
    args: tuple[re.Pattern[str], ...]
    secrets: dict[str, str]  # ENV name -> exact Bitwarden item name
    timeout: int  # seconds

    def items(self) -> tuple[str, ...]:
        """Sorted unique `secrets.values()`."""
        return tuple(sorted(set(self.secrets.values())))


@dataclass(frozen=True)
class Config:
    user: str
    lock_after: int  # minutes
    export_provider: str | None
    export_profiles: frozenset[str]
    bw_server: str | None
    actions: dict[str, Action]


@dataclass(frozen=True)
class ListReq:
    pass


@dataclass(frozen=True)
class RunReq:
    action: str
    args: tuple[str, ...]
    cwd: str = ""
    session: str = ""


@dataclass(frozen=True)
class ExportReq:
    profiles: tuple[str, ...]
    spec: str
    cwd: str = ""
    session: str = ""


type Request = ListReq | RunReq | ExportReq


@dataclass(frozen=True)
class RunResult:
    exit: int  # signal death -> 128 + signal
    stdout: bytes
    stderr: bytes
    timed_out: bool = False
    cancelled: bool = False
    overflow: bool = False  # runner dropped bytes beyond its keep limit


type Answer = Literal["once", "grant", "deny", "timeout"]


@dataclass(frozen=True)
class Pending:
    action: Action
    args: tuple[str, ...]
    cwd: str
    session: str
    first_use: bool
    locked: bool


type State = Literal["ready", "locked", "granted", "unavailable"]


@dataclass(frozen=True)
class ActionInfo:
    name: str
    description: str
    args: tuple[str, ...]  # regex source strings
    items: tuple[str, ...]
    state: State
    detail: str  # "" | "<n> min left" | item error reason


@dataclass(frozen=True)
class Entry:
    """One secretspec entry, allowlisted fields only."""

    key: str
    description: str | None = None
    required: bool | None = None
    default: str | None = None
    composed: str | None = None


@dataclass(frozen=True)
class Denied:
    reason: str


@dataclass(frozen=True)
class Failed:
    reason: str  # wire status "error"


@dataclass(frozen=True)
class Ran:
    exit: int
    stdout: bytes
    stderr: bytes
    truncated: bool


@dataclass(frozen=True)
class Listed:
    actions: tuple[ActionInfo, ...]


@dataclass(frozen=True)
class Exported:
    profiles: dict[str, dict[str, str]]


type Response = Denied | Failed | Ran | Listed | Exported
