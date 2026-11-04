from .data import Json, Obj


class ItemError(Exception):
    """Item missing, ambiguous or without a value; the message is the reason."""


def _value(item: Obj) -> Json:
    match item.get("type"):
        case 1:
            login = item.get("login")
            return login.get("password") if isinstance(login, dict) else None
        case 2:
            return item.get("notes")
        case _:
            return None


def pick(results: list[Obj], name: str) -> str:
    """The value of the one `bw list items --search` entry named exactly `name`."""
    found = [r for r in results if r.get("name") == name]
    if not found:
        raise ItemError(f"item missing: {name}")
    if len(found) > 1:
        raise ItemError(f"item ambiguous: {name}")
    value = _value(found[0])
    if not isinstance(value, str) or not value:
        raise ItemError(f"item has no value: {name}")
    return value
