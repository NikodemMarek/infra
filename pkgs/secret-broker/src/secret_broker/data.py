"""Typed boundary for untyped data: parsing and checked field access."""

import base64
import json
import math
import tomllib
from typing import NoReturn, cast

type Json = None | bool | int | float | str | list[Json] | dict[str, Json]
type Obj = dict[str, Json]


class ParseError(Exception):
    pass


def _reject_constant(name: str) -> NoReturn:
    raise ParseError(f"invalid json: {name} is not allowed")


def _parse_float(text: str) -> float:
    value = float(text)
    if not math.isfinite(value):
        raise ParseError("invalid json: number out of range")
    return value


def loads(data: str | bytes) -> Json:
    """Strict: NaN/Infinity, overflowing floats and text that cannot be encoded as UTF-8
    (lone surrogates) are refused."""
    try:
        value = cast(Json, json.loads(data, parse_constant=_reject_constant, parse_float=_parse_float))
        dumps(value).encode()
    except (ValueError, RecursionError) as e:
        raise ParseError(f"invalid json: {e}") from e
    return value


def dumps(value: Json) -> str:
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"))


def _check_toml(value: object) -> None:
    if isinstance(value, dict):
        for v in value.values():
            _check_toml(v)
    elif isinstance(value, list):
        for v in value:
            _check_toml(v)
    elif not isinstance(value, bool | int | float | str):
        raise ParseError("unsupported TOML value")


def toml_loads(text: str) -> Obj:
    """Any date/time value anywhere is refused: ParseError("unsupported TOML value")."""
    try:
        value = tomllib.loads(text)
    except (tomllib.TOMLDecodeError, ValueError, RecursionError) as e:
        raise ParseError("invalid TOML") from e
    _check_toml(value)
    return cast(Obj, value)


def as_obj(value: Json, what: str = "value") -> Obj:
    if isinstance(value, dict):
        return value
    raise ParseError(f"{what}: expected object, got {type(value).__name__}")


def as_list(value: Json, what: str = "value") -> list[Json]:
    if isinstance(value, list):
        return value
    raise ParseError(f"{what}: expected array, got {type(value).__name__}")


def get_str(o: Obj, key: str) -> str:
    v = o.get(key)
    if isinstance(v, str):
        return v
    raise ParseError(f"{key}: expected string, got {type(v).__name__}")


def opt_str(o: Obj, key: str, default: str | None) -> str | None:
    """An explicit null counts as absent."""
    v = o.get(key)
    if v is None:
        return default
    if isinstance(v, str):
        return v
    raise ParseError(f"{key}: expected string, got {type(v).__name__}")


def opt_int(o: Obj, key: str, default: int) -> int:
    """bool is not int here."""
    if key not in o:
        return default
    v = o[key]
    if isinstance(v, int) and not isinstance(v, bool):
        return v
    raise ParseError(f"{key}: expected integer, got {type(v).__name__}")


def opt_bool(o: Obj, key: str, default: bool) -> bool:
    if key not in o:
        return default
    v = o[key]
    if isinstance(v, bool):
        return v
    raise ParseError(f"{key}: expected boolean, got {type(v).__name__}")


def str_list(value: Json, what: str) -> list[str]:
    out: list[str] = []
    for v in as_list(value, what):
        if not isinstance(v, str):
            raise ParseError(f"{what}: expected string, got {type(v).__name__}")
        out.append(v)
    return out


def str_dict(value: Json, what: str) -> dict[str, str]:
    out: dict[str, str] = {}
    for k, v in as_obj(value, what).items():
        if not isinstance(v, str):
            raise ParseError(f"{what}.{k}: expected string, got {type(v).__name__}")
        out[k] = v
    return out


def b64(data: bytes) -> str:
    """Standard alphabet, padded."""
    return base64.b64encode(data).decode("ascii")


def unb64(text: str) -> bytes:
    try:
        return base64.b64decode(text, validate=True)
    except ValueError as e:  # binascii.Error, or non-ASCII text
        raise ParseError(f"invalid base64: {e}") from e
