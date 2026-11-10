import hashlib
import os
import stat
import sys
from pathlib import Path

import pytest

from matrix_inbox.data import Obj, loads
from matrix_inbox.io import ProcessTasks, SeenDir


def test_seen_first_claim_true_second_false(tmp_path: Path) -> None:
    seen = SeenDir(str(tmp_path / "commands"))
    assert seen.claim("$e1") is True
    assert seen.claim("$e1") is False


def test_seen_other_id_is_independent(tmp_path: Path) -> None:
    seen = SeenDir(str(tmp_path / "commands"))
    assert seen.claim("$e1") is True
    assert seen.claim("$e2") is True
    assert seen.claim("$e2") is False
    assert seen.claim("$e1") is False


def test_seen_works_across_instances(tmp_path: Path) -> None:
    path = str(tmp_path / "commands")
    assert SeenDir(path).claim("$e1") is True
    assert SeenDir(path).claim("$e1") is False
    assert SeenDir(path).claim("$e2") is True


def test_seen_creates_nested_directory_privately(tmp_path: Path) -> None:
    path = tmp_path / "a" / "b" / "commands"
    assert SeenDir(str(path)).claim("$e1") is True
    assert path.is_dir()
    assert stat.S_IMODE(path.stat().st_mode) == 0o700


def test_seen_existing_directory_is_fine(tmp_path: Path) -> None:
    path = tmp_path / "commands"
    path.mkdir()
    assert SeenDir(str(path)).claim("$e1") is True


def test_seen_marker_is_the_sha256_of_the_id(tmp_path: Path) -> None:
    path = tmp_path / "commands"
    SeenDir(str(path)).claim("$ev/ent:é")
    name = hashlib.sha256("$ev/ent:é".encode("utf-8")).hexdigest()
    assert os.listdir(path) == [name]
    assert stat.S_IMODE((path / name).stat().st_mode) == 0o600


def test_seen_ids_with_path_characters_stay_inside(tmp_path: Path) -> None:
    path = tmp_path / "commands"
    seen = SeenDir(str(path))
    assert seen.claim("../x") is True
    assert seen.claim("x") is True
    assert not (tmp_path / "x").exists()


def test_seen_unwritable_location_raises_oserror(tmp_path: Path) -> None:
    blocker = tmp_path / "file"
    blocker.write_text("x")
    with pytest.raises(OSError):
        SeenDir(str(blocker / "commands")).claim("$e1")


def install_stub(directory: Path, body: str, monkeypatch: pytest.MonkeyPatch) -> Path:
    """An `event-handler-task` first on PATH: records its stdin to `stdin.txt` next to it, then runs `body` (python)."""
    directory.mkdir(parents=True, exist_ok=True)
    record = directory / "stdin.txt"
    script = directory / "event-handler-task"
    script.write_text(
        f"#!{sys.executable}\n"
        "import sys\n"
        f"open({str(record)!r}, 'w', encoding='utf-8').write(sys.stdin.read())\n"
        f"{body}\n"
    )
    script.chmod(0o755)
    monkeypatch.setenv("PATH", f"{directory}{os.pathsep}{os.environ.get('PATH', '')}")
    return record


def test_tasks_stdin_and_parsed_outcome(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    record = install_stub(
        tmp_path / "bin",
        'print(\'{"type":"task.started","project":"infra","shpool":"s-1"}\')',
        monkeypatch,
    )
    outcome = ProcessTasks().start("fix it")
    assert outcome == {"type": "task.started", "project": "infra", "shpool": "s-1"}
    assert loads(record.read_text(encoding="utf-8")) == {"type": "task.new", "prompt": "fix it"}


def test_tasks_prompt_with_newlines_and_unicode(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    record = install_stub(tmp_path / "bin", "print('{}')", monkeypatch)
    prompt = 'ż\nó "q" — \\ end'
    assert ProcessTasks().start(prompt) == {}
    expected: Obj = {"type": "task.new", "prompt": prompt}
    assert loads(record.read_text(encoding="utf-8")) == expected


def test_tasks_uses_the_last_non_empty_line(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    install_stub(
        tmp_path / "bin",
        'print("noise")\nprint(\'{"type":"task.failed","reason":"x"}\')\nprint()\nprint("  ")',
        monkeypatch,
    )
    assert ProcessTasks().start("p") == {"type": "task.failed", "reason": "x"}


def test_tasks_exit_code_is_ignored(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    install_stub(tmp_path / "bin", 'print(\'{"type":"task.failed","reason":"empty task"}\')\nsys.exit(1)', monkeypatch)
    assert ProcessTasks().start("p") == {"type": "task.failed", "reason": "empty task"}


def test_tasks_stderr_is_not_the_result(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    install_stub(tmp_path / "bin", 'print(\'{"a":1}\')\nprint(\'{"b":2}\', file=sys.stderr)', monkeypatch)
    assert ProcessTasks().start("p") == {"a": 1}


@pytest.mark.parametrize(
    "body",
    ['print("not json")', 'print("[1]")', 'print("5")', 'print("null")', 'print(\'"s"\')', "pass", 'print("\\n\\n  \\n")'],
)
def test_tasks_non_object_or_empty_output_is_none(body: str, tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    install_stub(tmp_path / "bin", body, monkeypatch)
    assert ProcessTasks().start("p") is None


def test_tasks_last_line_not_json_is_none_even_if_earlier_is(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    install_stub(tmp_path / "bin", 'print(\'{"a":1}\')\nprint("trailing garbage")', monkeypatch)
    assert ProcessTasks().start("p") is None


def test_tasks_invalid_utf8_output_does_not_raise(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    install_stub(tmp_path / "bin", "sys.stdout.buffer.write(b'\\xff\\xfe garbage\\n')", monkeypatch)
    assert ProcessTasks().start("p") is None


def test_tasks_missing_executable_is_none(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    empty = tmp_path / "empty"
    empty.mkdir()
    monkeypatch.setenv("PATH", str(empty))
    assert ProcessTasks().start("p") is None
