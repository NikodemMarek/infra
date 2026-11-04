import secrets
from typing import Literal

TAG_ALPHABET = "abcdefghjkmnpqrstuvwxyz"


def parse_answer(text: str) -> Literal["once", "grant", "deny", "retry"]:
    """`text.strip().lower()`: y once, g grant, n deny, anything else retry."""
    match text.strip().lower():
        case "y":
            return "once"
        case "g":
            return "grant"
        case "n":
            return "deny"
        case _:
            return "retry"


def new_tag() -> str:
    return "".join(secrets.choice(TAG_ALPHABET) for _ in range(4))
