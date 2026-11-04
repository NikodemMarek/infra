import pytest

from secret_broker.console import TAG_ALPHABET, new_tag, parse_answer


@pytest.mark.parametrize(
    ("text", "expected"),
    [
        ("y", "once"),
        (" Y \n", "once"),
        ("g", "grant"),
        ("G", "grant"),
        ("n", "deny"),
        ("N\n", "deny"),
        ("", "retry"),
        ("yes", "retry"),
        ("x", "retry"),
        ("   ", "retry"),
    ],
)
def test_parse_answer(text: str, expected: str) -> None:
    assert parse_answer(text) == expected


def test_alphabet() -> None:
    assert TAG_ALPHABET == "abcdefghjkmnpqrstuvwxyz"


def test_new_tag() -> None:
    tags = [new_tag() for _ in range(100)]
    assert all(len(t) == 4 for t in tags)
    assert all(c in TAG_ALPHABET for t in tags for c in t)
    assert len(set(tags)) > 1
