from pathlib import Path

import pytest

from secret_broker.cli import main

STORE = "/nix/store/abc-tool/bin/tool"


def write(tmp_path: Path, text: str) -> str:
    p = tmp_path / "c.json"
    p.write_text(text)
    return str(p)


def test_check_ok(tmp_path: Path, capsys: pytest.CaptureFixture[str]) -> None:
    path = write(
        tmp_path,
        '{"user":"u","actions":{"x":{"description":"d","command":["' + STORE + '"]}}}',
    )
    assert main(["check", path]) == 0
    out = capsys.readouterr()
    assert out.out == ""
    assert out.err == ""


def test_check_bad_config(tmp_path: Path, capsys: pytest.CaptureFixture[str]) -> None:
    path = write(
        tmp_path,
        '{"user":"u","actions":{"x":{"description":"d","command":["/bin/sh"]}}}',
    )
    assert main(["check", path]) == 1
    out = capsys.readouterr()
    assert out.out == ""
    assert out.err.startswith("secret-broker: ")
    assert "x" in out.err


def test_check_invalid_json(tmp_path: Path, capsys: pytest.CaptureFixture[str]) -> None:
    assert main(["check", write(tmp_path, "{")]) == 1
    assert capsys.readouterr().err.startswith("secret-broker: ")


def test_check_missing_file(tmp_path: Path, capsys: pytest.CaptureFixture[str]) -> None:
    assert main(["check", str(tmp_path / "nope.json")]) == 1
    assert capsys.readouterr().err.startswith("secret-broker: ")


@pytest.mark.parametrize(
    "argv",
    [[], ["check"], ["check", "a", "b"], ["bogus"], ["bogus", "x"]],
)
def test_usage_error(argv: list[str]) -> None:
    assert main(argv) == 64
