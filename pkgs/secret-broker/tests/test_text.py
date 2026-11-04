import pytest

from secret_broker.text import clean, clean_block

TRUNC = "\n… (truncated)"


@pytest.mark.parametrize(
    ("raw", "expected"),
    [
        ("a\x1b[2Jb", "a?[2Jb"),
        ("x\ny\tz", "x?y?z"),
        ("‮evil", "?evil"),
        ("a​b", "a?b"),
        ("plain é text", "plain é text"),
        ("", ""),
    ],
)
def test_clean(raw: str, expected: str) -> None:
    assert clean(raw) == expected


def test_clean_limit() -> None:
    assert clean("a" * 300) == "a" * 199 + "…"
    assert clean("a" * 200) == "a" * 200
    assert clean("abcdef", 4) == "abc…"


def test_clean_block_plain() -> None:
    assert clean_block("a\nb") == "a\nb"


def test_clean_block_control() -> None:
    assert clean_block("a\x1b\nb") == "a?\nb"


def test_clean_block_lines() -> None:
    out = clean_block("\n".join(["x"] * 50))
    assert out == "\n".join(["x"] * 40) + TRUNC


def test_clean_block_exact_lines_not_truncated() -> None:
    s = "\n".join(["x"] * 40)
    assert clean_block(s) == s


def test_clean_block_chars() -> None:
    assert clean_block("y" * 5000) == "y" * 4000 + TRUNC


def test_clean_block_no_line_limit() -> None:
    assert clean_block("z" * 3000) == "z" * 3000
