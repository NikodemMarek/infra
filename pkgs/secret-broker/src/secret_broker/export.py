"""`secretspec export` of an already canonicalised spec, one profile at a time."""

import subprocess
from collections.abc import Mapping, Sequence

from . import data
from .data import ParseError
from .runner import child_dir

TIMEOUT = 300


class ExportError(Exception):
    pass


def run_export(
    secretspec: str,
    spec_text: str,
    profiles: Sequence[str],
    session: str,
    env: Mapping[str, str],
) -> dict[str, dict[str, str]]:
    """Profile -> secretspec's exported key/value pairs; `spec_text` is always `canonical_spec` output."""
    out: dict[str, dict[str, str]] = {}
    for p in profiles:
        # a fresh directory per profile: HOME must be empty for every child
        with child_dir() as d:
            (d / "secretspec.toml").write_text(spec_text, encoding="utf-8")
            try:
                r = subprocess.run(
                    [secretspec, "export", "--profile", p, "--format", "json"],
                    cwd=d,
                    env={**env, "BW_SESSION": session, "HOME": str(d), "TMPDIR": str(d)},
                    stdin=subprocess.DEVNULL,
                    capture_output=True,
                    timeout=TIMEOUT,
                    check=False,
                )
            except (OSError, subprocess.TimeoutExpired) as e:
                raise ExportError(f"secretspec export failed for {p} ({type(e).__name__})") from e
        if r.returncode != 0:
            raise ExportError(f"secretspec export failed for {p} (exit {r.returncode})")
        try:
            out[p] = data.str_dict(data.loads(r.stdout), p)
        except ParseError as e:
            raise ExportError(f"secretspec export returned invalid JSON for {p}") from e
    return out
