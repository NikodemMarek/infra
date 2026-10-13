"""SCOPES.toml: which scopes apply to a directory and the budgets (S3)."""

import os
import re
from collections.abc import Collection, Sequence
from dataclasses import dataclass
from pathlib import Path

from . import data
from .data import Json, Obj, ParseError
from .entry import SCOPE_RE, EntryError
from .store import ProjectRow

SCOPES_FILE = "SCOPES.toml"

DEFAULT_SCOPES_TOML = """\
# Knowledge scopes: data file of the knowledge curator, edit freely, then run `knowledge lint`.
# [[path]]: a cwd under `prefix` gets `scopes`. [[marker]]: a file or dir in the repo root (or the cwd) gives `scopes`.
# [budget]: [entries, bytes] per scope.
# Example [[path]] (none by default):
# [[path]]
# prefix = "~/projects/work"
# scopes = ["org/work"]

[[marker]]
file = "tsconfig.json"
scopes = ["lang/typescript"]
[[marker]]
file = "package.json"
scopes = ["lang/javascript"]
[[marker]]
file = "pyproject.toml"
scopes = ["lang/python"]
[[marker]]
file = "flake.nix"
scopes = ["lang/nix"]
[[marker]]
file = "pom.xml"
scopes = ["lang/java"]
[[marker]]
file = "build.gradle"
scopes = ["lang/java"]
[[marker]]
file = "Cargo.toml"
scopes = ["lang/rust"]
[[marker]]
file = "go.mod"
scopes = ["lang/go"]
[[marker]]
file = ".jj"
scopes = ["topic/jj"]
[[marker]]
file = ".gitlab-ci.yml"
scopes = ["topic/gitlab"]
[[marker]]
file = "Dockerfile"
scopes = ["topic/docker"]
[[marker]]
file = "kustomization.yaml"
scopes = ["topic/kubernetes"]

[budget]
global = [40, 65536]
lang = [40, 65536]
topic = [30, 49152]
org = [80, 393216]
project = [25, 49152]
total = [300, 1048576]
"""

# the keys of [budget]: the scope kinds, and the whole store
BUDGET_KEYS = ("global", "lang", "topic", "org", "project", "total")
DEFAULT_BUDGET: dict[str, tuple[int, int]] = {
    "global": (40, 65536),
    "lang": (40, 65536),
    "topic": (30, 49152),
    "org": (80, 393216),
    "project": (25, 49152),
    "total": (300, 1048576),
}

_SCOPE = re.compile(SCOPE_RE)
_AGENTS = ".agents"


@dataclass(frozen=True)
class PathRule:
    prefix: str  # may start with ~
    scopes: tuple[str, ...]


@dataclass(frozen=True)
class MarkerRule:
    file: str  # a file or directory name
    scopes: tuple[str, ...]


@dataclass(frozen=True)
class ScopesConfig:
    paths: tuple[PathRule, ...]
    markers: tuple[MarkerRule, ...]
    budgets: dict[str, tuple[int, int]]  # (entries, bytes) per scope, keyed by global|lang|topic|org|project
    total_budget: tuple[int, int]

    def budget_for(self, scope: str) -> tuple[int, int]:
        """(entries, bytes) a scope may hold."""
        if _SCOPE.fullmatch(scope) is None:
            raise EntryError(f"invalid scope {scope!r}")
        kind = "project" if scope.startswith("projects/") else scope.split("/", 1)[0]
        return self.budgets[kind]


# loading


def _only(o: Obj, allowed: tuple[str, ...], what: str) -> None:
    extra = [k for k in o if k not in allowed]
    if extra:
        raise ParseError(f"{what}: unknown keys {', '.join(extra)}")


def _rule_scopes(o: Obj, what: str) -> tuple[str, ...]:
    scopes = data.str_list(o, "scopes")
    if not scopes:
        raise ParseError(f"{what}: scopes must list at least one scope")
    for s in scopes:
        if _SCOPE.fullmatch(s) is None:
            raise ParseError(f"{what}: invalid scope {s!r}")
    return tuple(scopes)


def _pos_int(value: Json, what: str) -> int:
    if isinstance(value, int) and not isinstance(value, bool) and value > 0:
        return value
    raise ParseError(f"{what}: expected a positive integer")


def _pair(value: Json, what: str) -> tuple[int, int]:
    items = data.as_list(value, what)
    if len(items) != 2:
        raise ParseError(f"{what}: expected [entries, bytes]")
    return _pos_int(items[0], what), _pos_int(items[1], what)


def parse_scopes(text: str) -> ScopesConfig:
    """A SCOPES.toml; budgets that the file leaves out keep their defaults. Raises ParseError."""
    doc = data.load_toml(text)
    _only(doc, ("path", "marker", "budget"), "SCOPES.toml")
    paths: list[PathRule] = []
    for i, item in enumerate(data.opt_list(doc, "path")):
        what = f"path[{i}]"
        o = data.as_obj(item, what)
        _only(o, ("prefix", "scopes"), what)
        prefix = data.get_str(o, "prefix")
        if not prefix:
            raise ParseError(f"{what}: empty prefix")
        paths.append(PathRule(prefix, _rule_scopes(o, what)))
    markers: list[MarkerRule] = []
    for i, item in enumerate(data.opt_list(doc, "marker")):
        what = f"marker[{i}]"
        o = data.as_obj(item, what)
        _only(o, ("file", "scopes"), what)
        file = data.get_str(o, "file")
        if not file or "/" in file:
            raise ParseError(f"{what}: file must be a plain name, got {file!r}")
        markers.append(MarkerRule(file, _rule_scopes(o, what)))
    budgets = dict(DEFAULT_BUDGET)
    given = data.opt_obj(doc, "budget") or {}
    _only(given, BUDGET_KEYS, "budget")
    for key, value in given.items():
        budgets[key] = _pair(value, f"budget.{key}")
    total = budgets.pop("total")
    return ScopesConfig(tuple(paths), tuple(markers), budgets, total)


def load_scopes(memory: Path) -> ScopesConfig:
    """`<memory>/SCOPES.toml`, or the defaults if there is none. Raises ParseError if it is invalid."""
    try:
        text = (memory / SCOPES_FILE).read_text(encoding="utf-8")
    except FileNotFoundError:
        text = DEFAULT_SCOPES_TOML
    except UnicodeDecodeError as e:
        raise ParseError(f"{SCOPES_FILE}: not UTF-8 text") from e
    try:
        return parse_scopes(text)
    except ParseError as e:
        raise ParseError(f"{SCOPES_FILE}: {e}") from e


# scopes for a directory


def normalise_cwd(cwd: Path) -> Path:
    """A jj workspace `X.agents/<ws>/...` is `X/...` (the agents' workspaces stand for their repo)."""
    parts = Path(os.path.abspath(cwd)).parts
    out: list[str] = []
    i = 0
    while i < len(parts):
        part = parts[i]
        if part.endswith(_AGENTS) and part != _AGENTS and i + 1 < len(parts):
            out.append(part.removesuffix(_AGENTS))
            i += 2
        else:
            out.append(part)
            i += 1
    return Path(*out)


def repo_root(cwd: Path) -> Path:
    """The nearest ancestor (or cwd itself) with `.jj` or `.git`; the cwd if there is none."""
    for d in (cwd, *cwd.parents):
        if (d / ".jj").exists() or (d / ".git").exists():
            return d
    return cwd


def _under(path: Path, prefix: Path) -> bool:
    return path == prefix or prefix in path.parents


def _order(scope: str) -> tuple[int, str]:
    """Projects, org, lang and topic, global."""
    if scope.startswith("projects/"):
        return 0, scope
    if scope.startswith("org/"):
        return 1, scope
    if scope == "global":
        return 3, scope
    return 2, scope


def project_for_cwd(cwd: Path, projects: Sequence[ProjectRow]) -> ProjectRow | None:
    """The project whose repo is the longest prefix of `cwd` (normalised: an agents' workspace is its repo)."""
    cwd = normalise_cwd(cwd)
    best: ProjectRow | None = None
    best_depth = -1
    for row in projects:
        repo = Path(row.repo).expanduser()
        if _under(cwd, repo) and len(repo.parts) > best_depth:
            best, best_depth = row, len(repo.parts)
    return best


def scopes_for_cwd(
    cwd: Path, cfg: ScopesConfig, projects: Sequence[ProjectRow], slugs: Collection[str]
) -> list[str]:
    """The scopes whose knowledge applies in `cwd`: project, org, lang/topic (sorted), global. No duplicates."""
    best = project_for_cwd(cwd, projects)
    cwd = normalise_cwd(cwd)
    root = repo_root(cwd)
    found: set[str] = {"global"}
    project = f"projects/{best.slug}/facts" if best is not None and best.slug in slugs else None
    for rule in cfg.paths:
        if _under(cwd, Path(rule.prefix).expanduser()):
            found.update(rule.scopes)
    for marker in cfg.markers:
        if (root / marker.file).exists() or (cwd / marker.file).exists():
            found.update(marker.scopes)
    found.discard(project or "")
    rest = sorted(found, key=_order)
    return [project, *rest] if project else rest
