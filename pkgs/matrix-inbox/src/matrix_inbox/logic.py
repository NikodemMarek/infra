"""Pure logic: making untrusted text safe to post."""

import unicodedata


def sanitize(text: str) -> str:
    text = text.replace("\n", " ").replace("\t", " ")
    return "".join(
        "�" if unicodedata.category(c) in ("Cc", "Cf", "Zl", "Zp") else c for c in text
    )
