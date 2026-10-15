import io

import pytest

from knowledge import cli


class Tty(io.StringIO):
    def isatty(self) -> bool:
        return True


class Runner:
    def __init__(self, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]) -> None:
        self.monkeypatch = monkeypatch
        self.capsys = capsys

    def __call__(self, *argv: str, stdin: str = "", tty: bool = False) -> tuple[int, str]:
        """Run `knowledge <argv>` with `stdin` piped in, or typed at a terminal: the exit code and stdout."""
        self.monkeypatch.setattr("sys.argv", ["knowledge", *argv])
        self.monkeypatch.setattr("sys.stdin", Tty(stdin) if tty else io.StringIO(stdin))
        code = 0
        try:
            cli.main()
        except SystemExit as e:
            code = int(e.code or 0)
        return code, self.capsys.readouterr().out
