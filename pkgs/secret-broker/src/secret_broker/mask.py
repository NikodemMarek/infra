from collections.abc import Iterable

MASK = b"[masked]"
MIN_LEN = 6


def mask(data: bytes, values: Iterable[bytes]) -> bytes:
    """Raw replace, longest value first; values shorter than MIN_LEN are skipped."""
    for v in sorted({v for v in values if len(v) >= MIN_LEN}, key=len, reverse=True):
        data = data.replace(v, MASK)
    return data
