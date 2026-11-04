"""The Bitwarden CLI: every call runs with a fresh HOME and TMPDIR; password and session only in env."""

import subprocess
from collections.abc import Iterator, Mapping, Sequence
from contextlib import contextmanager, suppress
from pathlib import Path

from . import data
from .data import Obj, ParseError
from .runner import child_dir
from .vault import ItemError, pick


class BwError(Exception):
    pass


def _child_env(env: Mapping[str, str], d: Path) -> dict[str, str]:
    return {**env, "HOME": str(d), "TMPDIR": str(d)}


def _run(argv: Sequence[str], env: Mapping[str, str], timeout: float = 60) -> subprocess.CompletedProcess[bytes]:
    with child_dir() as d:
        try:
            return subprocess.run(
                argv,
                env=_child_env(env, d),
                cwd=d,
                stdin=subprocess.DEVNULL,
                capture_output=True,
                timeout=timeout,
                check=False,
            )
        except (OSError, subprocess.TimeoutExpired) as e:
            raise BwError(f"bw {argv[1]} failed: {type(e).__name__}") from e


def status(bw: str, env: Mapping[str, str]) -> Obj:
    """`[bw, "status"]` parsed as a JSON object."""
    r = _run([bw, "status"], env)
    if r.returncode != 0:
        raise BwError("bw status failed")
    try:
        return data.as_obj(data.loads(r.stdout), "bw status")
    except ParseError as e:
        raise BwError("bw status returned invalid JSON") from e


def check_server(st: Obj, expected: str | None) -> None:
    """`st["serverUrl"]` (null = default) must equal `expected`, else `BwError(f"bw server is {got}, expected {expected}")`."""
    got = st.get("serverUrl")
    if got != expected:
        raise BwError(f"bw server is {got}, expected {expected}")


def config_server(bw: str, env: Mapping[str, str], url: str) -> None:
    """`bw config server <url>`: a fresh bw has to be pointed at the server before `login`."""
    if _run([bw, "config", "server", url], env).returncode != 0:
        raise BwError("bw config server failed")


def login(bw: str, env: Mapping[str, str], tty_in: int, tty_out: int) -> None:
    """`bw login --apikey` attached to the console tty."""
    with child_dir() as d:
        try:
            r = subprocess.run(
                [bw, "login", "--apikey"],
                env=_child_env(env, d),
                cwd=d,
                stdin=tty_in,
                stdout=tty_out,
                stderr=tty_out,
                timeout=300,
                check=False,
            )
        except (OSError, subprocess.TimeoutExpired) as e:
            raise BwError(f"bw login failed: {type(e).__name__}") from e
    if r.returncode != 0:
        raise BwError("bw login failed")


@contextmanager
def unlocked(bw: str, password: str, env: Mapping[str, str]) -> Iterator[str]:
    """Yield the session of `bw unlock`; always run `bw lock` on exit. A failed unlock is `BwError("wrong master password")`."""
    r = _run([bw, "unlock", "--passwordenv", "BW_PASSWORD", "--raw"], {**env, "BW_PASSWORD": password})
    session = r.stdout.decode(errors="replace").strip()
    if r.returncode != 0 or not session:
        raise BwError("wrong master password")
    try:
        yield session
    finally:
        # best effort: must not hide the error of the body
        with suppress(BwError):
            _run([bw, "lock"], env)


def sync(bw: str, session: str, env: Mapping[str, str], timeout: float = 15) -> str | None:
    """None when ok, else `sync failed, data from <lastSync|never>`."""
    with suppress(BwError):
        if _run([bw, "sync"], {**env, "BW_SESSION": session}, timeout).returncode == 0:
            return None
    last: data.Json = None
    with suppress(BwError):
        last = status(bw, env).get("lastSync")
    return f"sync failed, data from {last if isinstance(last, str) else 'never'}"


def load(
    bw: str, session: str, names: Sequence[str], env: Mapping[str, str]
) -> tuple[dict[str, str], dict[str, str]]:
    """Values and errors (reasons) per item name, via `bw list items --search` and `vault.pick`."""
    values: dict[str, str] = {}
    errors: dict[str, str] = {}
    for name in names:
        r = _run([bw, "list", "items", "--search", name], {**env, "BW_SESSION": session})
        if r.returncode != 0:
            raise BwError("bw list failed")
        try:
            results = [data.as_obj(o, "item") for o in data.as_list(data.loads(r.stdout), "bw list")]
        except ParseError as e:
            raise BwError("bw list returned invalid JSON") from e
        try:
            values[name] = pick(results, name)
        except ItemError as e:
            errors[name] = str(e)
    return values, errors
