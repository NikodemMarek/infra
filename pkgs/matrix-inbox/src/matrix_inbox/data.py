"""Typed boundary for untyped json: parsing, dumping and checked field access.

Nothing else in the package imports json.
"""

import json
from typing import cast

type Json = None | bool | int | float | str | list[Json] | dict[str, Json]
type Obj = dict[str, Json]


class ParseError(Exception):
    pass


def loads(data: str | bytes) -> Json:
    try:
        return cast(Json, json.loads(data))
    except ValueError as e:
        raise ParseError(f"invalid json: {e}") from e


def dumps(value: Json, indent: int | None = None, sort_keys: bool = False) -> str:
    """Compact (one line) without indent."""
    if indent is None:
        return json.dumps(value, ensure_ascii=False, separators=(",", ":"), sort_keys=sort_keys)
    return json.dumps(value, ensure_ascii=False, indent=indent, sort_keys=sort_keys)


def get_str(o: Obj, key: str) -> str | None:
    v = o.get(key)
    return v if isinstance(v, str) else None


def get_obj(o: Obj, key: str) -> Obj | None:
    v = o.get(key)
    return v if isinstance(v, dict) else None


def get_list(o: Obj, key: str) -> list[Json] | None:
    v = o.get(key)
    return v if isinstance(v, list) else None
