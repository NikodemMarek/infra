"""Typed boundary for untyped data (json, toml): parsing and checked field access.

Nothing else in the package imports json or tomllib.
"""

import datetime
import json
import re
import sys
import tomllib
from typing import cast

type Json = None | bool | int | float | str | list[Json] | dict[str, Json]
type Obj = dict[str, Json]


class ParseError(Exception):
    pass


def _clean_str(s: str) -> str:
    """A lone surrogate (JSON `"\\ud800"`) cannot be written out: it becomes `?`."""
    return s if s.isascii() else s.encode("utf-8", "replace").decode()


def _clean(value: Json) -> Json:
    if isinstance(value, str):
        return _clean_str(value)
    if isinstance(value, list):
        return [_clean(v) for v in value]
    if isinstance(value, dict):
        return {_clean_str(k): _clean(v) for k, v in value.items()}
    return value


def _normalize(value: object) -> Json:
    """Any parsed document as Json: dates and datetimes become ISO strings, anything else foreign is an error."""
    if value is None or isinstance(value, (bool, int, float, str)):
        return value
    if isinstance(value, (datetime.date, datetime.time)):  # datetime.datetime is a date
        return value.isoformat()
    if isinstance(value, (list, tuple)):
        return [_normalize(v) for v in value]
    if isinstance(value, dict):
        out: Obj = {}
        for k, v in value.items():
            if not isinstance(k, str):
                raise ParseError(f"keys must be strings, got {k!r}")
            out[k] = _normalize(v)
        return out
    raise ParseError(f"unsupported value of type {type(value).__name__}")


def loads(data: str | bytes) -> Json:
    try:
        return _clean(cast(Json, json.loads(data)))
    except json.JSONDecodeError as e:
        raise ParseError(f"invalid json: {e}") from e


def loads_from(text: str, start: int) -> Json:
    """The JSON document that begins at `start` in text; what follows it is ignored."""
    try:
        return _clean(cast(Json, json.JSONDecoder().raw_decode(text, start)[0]))
    except json.JSONDecodeError as e:
        raise ParseError(f"invalid json: {e}") from e


def dumps(value: Json, indent: int | None = None) -> str:
    if indent is None:
        return json.dumps(value, ensure_ascii=False, separators=(",", ":"))
    return json.dumps(value, ensure_ascii=False, indent=indent)


def load_toml(text: str) -> Obj:
    try:
        return as_obj(_clean(_normalize(tomllib.loads(text))), "toml")
    except tomllib.TOMLDecodeError as e:
        raise ParseError(f"invalid toml: {e}") from e


def read_stdin() -> str:
    """Piped stdin, or "" for a terminal.

    sys.stdin is typed as possibly Any.
    """
    if sys.stdin is None or sys.stdin.isatty():
        return ""
    data: str = sys.stdin.read()
    return data


def group(m: re.Match[str], n: int = 0) -> str | None:
    """Match.group() is typed `str | Any`."""
    return cast(str | None, m.group(n))


def as_obj(value: Json, what: str = "value") -> Obj:
    if isinstance(value, dict):
        return value
    raise ParseError(f"{what}: expected object, got {type(value).__name__}")


def as_list(value: Json, what: str = "value") -> list[Json]:
    if isinstance(value, list):
        return value
    raise ParseError(f"{what}: expected array, got {type(value).__name__}")


def opt_list(o: Obj, key: str) -> list[Json]:
    """Missing or null is an empty list."""
    v = o.get(key)
    return [] if v is None else as_list(v, key)


def opt_str(o: Obj, key: str) -> str | None:
    v = o.get(key)
    if v is None or isinstance(v, str):
        return v
    raise ParseError(f"{key}: expected string, got {type(v).__name__}")


def get_str(o: Obj, key: str) -> str:
    v = opt_str(o, key)
    if v is None:
        raise ParseError(f"{key}: missing")
    return v


def str_or(o: Obj, key: str, default: str = "") -> str:
    v = opt_str(o, key)
    return default if v is None else v


def str_list(o: Obj, key: str) -> list[str]:
    out: list[str] = []
    for v in opt_list(o, key):
        if not isinstance(v, str):
            raise ParseError(f"{key}: expected strings")
        out.append(v)
    return out


def get_bool(o: Obj, key: str, default: bool = False) -> bool:
    v = o.get(key)
    if v is None:
        return default
    if isinstance(v, bool):
        return v
    raise ParseError(f"{key}: expected bool, got {type(v).__name__}")
