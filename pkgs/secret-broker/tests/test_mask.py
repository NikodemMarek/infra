from secret_broker.mask import MASK, MIN_LEN, mask


def test_constants() -> None:
    assert MASK == b"[masked]"
    assert MIN_LEN == 6


def test_basic() -> None:
    assert mask(b"tok=abcdef123", [b"abcdef123"]) == b"tok=[masked]"


def test_short_value_skipped() -> None:
    assert mask(b"x abc12 y", [b"abc12"]) == b"x abc12 y"


def test_exactly_min_len_masked() -> None:
    assert mask(b"x abcdef y", [b"abcdef"]) == b"x [masked] y"


def test_longest_first() -> None:
    assert mask(b"abcdefgh", [b"abcdef", b"abcdefgh"]) == b"[masked]"
    assert mask(b"abcdefgh", [b"abcdefgh", b"abcdef"]) == b"[masked]"


def test_no_values() -> None:
    assert mask(b"a\nb", []) == b"a\nb"


def test_two_occurrences() -> None:
    assert mask(b"abcdef-abcdef", [b"abcdef"]) == b"[masked]-[masked]"


def test_multiple_values_and_generator() -> None:
    out = mask(b"aaaaaa bbbbbb", (v for v in [b"aaaaaa", b"bbbbbb"]))
    assert out == b"[masked] [masked]"


def test_binary() -> None:
    assert mask(b"\x00\xff\xfe\xfd\xfc\xfb\xfa\x01", [b"\xff\xfe\xfd\xfc\xfb\xfa"]) == b"\x00[masked]\x01"
    assert mask(b"", [b"abcdef"]) == b""
