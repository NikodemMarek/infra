import unicodedata


def _sanitize(s: str) -> str:
    """Replace control and format characters, which could spoof the console, with `?`."""
    return "".join("?" if unicodedata.category(c) in ("Cc", "Cf") else c for c in s)


def clean(s: str, limit: int = 200) -> str:
    s = _sanitize(s)
    return s[: limit - 1] + "…" if len(s) > limit else s


def clean_block(s: str, max_lines: int = 40, max_chars: int = 4000) -> str:
    lines = s.split("\n")
    out = "\n".join(_sanitize(line) for line in lines[:max_lines])
    cut = len(lines) > max_lines or len(out) > max_chars
    return out[:max_chars] + "\n… (truncated)" if cut else out
