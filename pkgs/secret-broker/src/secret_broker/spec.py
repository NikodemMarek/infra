import json
import re
import urllib.parse
from collections.abc import Sequence
from collections.abc import Set as AbstractSet

from . import data
from .data import Json, Obj, ParseError
from .model import Entry

_FIELDS = frozenset({"description", "required", "default", "composed"})
_NAME = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")


class SpecError(Exception):
    """The message is the reason."""


def _q(s: str) -> str:
    """A TOML basic string. json.dumps alone is not valid TOML for DEL and for characters
    outside the BMP (it emits surrogate pairs)."""
    out: list[str] = []
    for c in s:
        if ord(c) == 0x7F:
            out.append("\\u007f")
        elif ord(c) > 0xFFFF:
            out.append(f"\\U{ord(c):08x}")
        else:
            out.append(json.dumps(c, ensure_ascii=True)[1:-1])
    return '"' + "".join(out) + '"'


def _typed[T](fields: Obj, ctx: str, name: str, kind: type[T]) -> T | None:
    v: object = fields.get(name)  # object, so isinstance narrows to T
    if v is None:
        return None
    if isinstance(v, kind):
        return v
    raise SpecError(f"spec rejected: {ctx}.{name} has the wrong type")


def _entry(profile: str, key: str, value: Json) -> Entry:
    ctx = f"{profile}.{key}"
    if not isinstance(value, dict):
        raise SpecError(f"spec rejected: {ctx} is not a table")
    if key == "defaults":
        raise SpecError(f"spec rejected: {ctx} is not allowed")
    if not _NAME.fullmatch(key):
        raise SpecError(f"spec rejected: {ctx} invalid name")
    for f in value:
        if f not in _FIELDS:
            raise SpecError(f"spec rejected: {ctx}.{f} is not allowed")
    return Entry(
        key=key,
        description=_typed(value, ctx, "description", str),
        required=_typed(value, ctx, "required", bool),
        default=_typed(value, ctx, "default", str),
        composed=_typed(value, ctx, "composed", str),
    )


def _check_composed(profile: str, entries: Sequence[Entry]) -> None:
    """A composed value may only reference other entries of its own profile."""
    keys = {e.key for e in entries}
    for e in entries:
        if e.composed is None:
            continue
        pos = 0
        while (start := e.composed.find("${", pos)) != -1:
            end = e.composed.find("}", start + 2)
            name = e.composed[start + 2 :] if end == -1 else e.composed[start + 2 : end]
            if end == -1 or name == e.key or name not in keys:
                raise SpecError(f"spec rejected: {profile}.{e.key}.composed references {name}")
            pos = end + 1


def _render_entry(e: Entry) -> str:
    fields: list[str] = []
    if e.description is not None:
        fields.append(f"description = {_q(e.description)}")
    if e.required is not None:
        fields.append(f"required = {'true' if e.required else 'false'}")
    if e.default is not None:
        fields.append(f"default = {_q(e.default)}")
    if e.composed is not None:
        fields.append(f"composed = {_q(e.composed)}")
    return f"{_q(e.key)} = {{ {', '.join(fields)} }}" if fields else f"{_q(e.key)} = {{}}"


def canonical_spec(
    text: str, profiles: Sequence[str], allowed: AbstractSet[str], provider: str
) -> tuple[str, dict[str, list[Entry]]]:
    """The broker's own secretspec.toml for `profiles`, rendered from the allowlisted
    fields of the caller's `text`, plus the entries per profile."""
    requested: list[str] = []
    for p in profiles:
        if p not in requested:
            requested.append(p)
    for p in requested:
        if p not in allowed:
            raise SpecError(f"profile not allowed: {p}")
    try:
        doc = data.toml_loads(text)
    except ParseError as e:
        raise SpecError("spec rejected: invalid TOML") from e
    tables = doc.get("profiles")
    entries: dict[str, list[Entry]] = {}
    for p in requested:
        table = tables.get(p) if isinstance(tables, dict) else None
        if table is None:
            raise SpecError(f"unknown profile: {p}")
        if not isinstance(table, dict):
            raise SpecError(f"spec rejected: profiles.{p} is not a table")
        entries[p] = [_entry(p, k, v) for k, v in table.items()]
        _check_composed(p, entries[p])
    blocks = [
        '[project]\nname = "secret-broker-export"\nrevision = "1.0"\nrequire_reason = false',
        f"[providers]\nbw = {_q(provider)}",
        '[defaults]\nproviders = ["bw"]',
    ]
    for p, es in entries.items():
        blocks.append("\n".join([f"[profiles.{_q(p)}]", *map(_render_entry, es)]))
    return "\n\n".join(blocks) + "\n", entries


def _inputs(composed: str) -> list[str]:
    """The `${NAME}` references in order of first appearance."""
    names: list[str] = []
    pos = 0
    while (start := composed.find("${", pos)) != -1:
        end = composed.find("}", start + 2)
        if end == -1:
            break
        name = composed[start + 2 : end]
        if name not in names:
            names.append(name)
        pos = end + 1
    return names


def resolved_titles(provider: str, profile: str, entries: Sequence[Entry]) -> list[str]:
    """The Bitwarden item title of each entry; composed entries are marked, no item."""
    query = urllib.parse.parse_qs(urllib.parse.urlsplit(provider).query)
    folder = query.get("folder", [""])[0].replace("{profile}", profile)
    titles: list[str] = []
    for e in entries:
        if e.composed is not None:
            titles.append(f"{e.key} (composed of {', '.join(_inputs(e.composed))})")
        else:
            titles.append(f"{folder}/{e.key}" if folder else e.key)
    return titles
