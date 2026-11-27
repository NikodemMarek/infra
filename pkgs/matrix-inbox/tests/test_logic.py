import pytest
from matrix_inbox import logic
from matrix_inbox.data import ParseError, loads


@pytest.mark.parametrize(
    ("text", "expected"),
    [
        ("a\x1bb", "a�b"),
        ("a‮b", "a�b"),  # noqa: PLE2502
        ("a\u200bb", "a�b"),
        ("a\x7fb", "a�b"),
        ("a\rb", "a�b"),
        ("a b", "a�b"),
        ("a b", "a�b"),
        ("a\nb\tc", "a b c"),
        ("a\x1b\rb\n", "a��b "),
        ("", ""),
        ("plain é text", "plain é text"),
    ],
)
def test_sanitize(text: str, expected: str) -> None:
    assert logic.sanitize(text) == expected


def test_sanitize_line_separators_are_replaced() -> None:
    assert logic.sanitize("a b c") == "a�b�c"


def test_loads_invalid_utf8_raises_parse_error() -> None:
    with pytest.raises(ParseError):
        loads(b"\xff\xfe{")
