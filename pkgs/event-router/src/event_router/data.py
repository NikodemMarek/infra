"""Typed boundary for untyped data: parsing and checked field access."""

import json
import sys
from typing import NoReturn, cast

type Json = None | bool | int | float | str | list[Json] | dict[str, Json]
type Obj = dict[str, Json]


class ParseError(Exception):
    pass


def _reject_constant(name: str) -> NoReturn:
    raise ParseError(f"invalid json: {name} is not allowed")


def loads(data: str | bytes) -> Json:
    """Strict: NaN/Infinity and text that cannot be encoded as UTF-8 (lone surrogates)
    are refused, as consumers of `dumps` (the inbox API) could not handle them."""
    try:
        value = cast(Json, json.loads(data, parse_constant=_reject_constant))
        dumps(value).encode()
    except (json.JSONDecodeError, UnicodeError) as e:
        raise ParseError(f"invalid json: {e}") from e
    return value


def dumps(value: Json) -> str:
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"))


def as_obj(value: Json, what: str = "value") -> Obj:
    if isinstance(value, dict):
        return value
    raise ParseError(f"{what}: expected object, got {type(value).__name__}")


def get_str(o: Obj, key: str) -> str:
    v = o.get(key)
    if isinstance(v, str):
        return v
    raise ParseError(f"{key}: expected string, got {type(v).__name__}")


def read_stdin() -> bytes:
    """Piped stdin, or b"" for a terminal (sys.stdin is typed as possibly Any)."""
    if sys.stdin is None or sys.stdin.isatty():
        return b""
    data: bytes = sys.stdin.buffer.read()
    return data
