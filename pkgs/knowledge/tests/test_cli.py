from pathlib import Path

import pytest

from editing_claude import EditingClaude
from knowledge import cli, notes
from runner import Runner

TOKEN = "glpat-" + "a" * 20


@pytest.fixture
def run(tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]) -> Runner:
    monkeypatch.chdir(tmp_path)  # not an event's `.agents/gl-*` workspace, whatever runs the tests
    return Runner(monkeypatch, capsys)


def test_no_command_is_a_usage_error(monkeypatch: pytest.MonkeyPatch):
    monkeypatch.setattr("sys.argv", ["knowledge"])
    with pytest.raises(SystemExit) as e:
        cli.main()
    assert e.value.code == 2


@pytest.mark.parametrize("command", ["paths", "init", "index", "lint", "ask", "receive", "import", "status", "undo", "restore", "drop"])
def test_the_removed_commands_are_gone(run: Runner, command: str):
    assert run(command)[0] == 2


# submit


def test_submit_queues_stdin_as_a_note_and_prints_its_path(run: Runner, tmp_path: Path):
    code, out = run("submit", stdin="title: t\n---\nbody\n")
    assert code == 0
    (path,) = notes.pending()
    assert out == f"{path}\n"
    assert path.read_text().splitlines()[1:] == ["title: t", "---", "body"]
    assert path.stat().st_mode & 0o777 == 0o600
    assert path.read_text().startswith(f"origin: user   cwd: {tmp_path}   date: ")


def test_submit_inside_claude_code_is_a_session_note(run: Runner, monkeypatch: pytest.MonkeyPatch):
    monkeypatch.setenv("CLAUDECODE", "1")
    assert run("submit", stdin="a note")[0] == 0
    assert [notes.origin_of(p) for p in notes.pending()] == ["session"]


def test_submit_from_an_event_session_is_an_event_note(run: Runner, monkeypatch: pytest.MonkeyPatch):
    monkeypatch.setenv("CLAUDECODE", "1")
    monkeypatch.setenv("KNOWLEDGE_ORIGIN", "event:gitlab.note")
    assert run("submit", stdin="a note")[0] == 0
    assert [notes.origin_of(p) for p in notes.pending()] == ["event:gitlab.note"]


def test_submit_refuses_a_secret(run: Runner, caplog: pytest.LogCaptureFixture):
    assert run("submit", stdin=f"title: t\n---\ntoken {TOKEN}\n") == (2, "")
    assert "refusing to submit" in caplog.text
    assert notes.pending() == []


def test_submit_needs_a_note_on_stdin(run: Runner):
    assert run("submit", stdin="  \n") == (2, "")
    assert run("submit", stdin="a note", tty=True) == (2, "")  # a terminal is not read
    assert notes.pending() == []


def test_submit_takes_no_flags(run: Runner):
    assert run("submit", "--title", "t", stdin="a note")[0] == 2
    assert notes.pending() == []


# curate and skills


def test_curate_runs_the_curator_and_exits_with_its_code(run: Runner, memory_repo: Path, editing_claude: EditingClaude):
    assert run("curate") == (0, "")  # nothing queued
    assert not editing_claude.called
    queued = notes.write("title: t\n---\nb", "user", Path("/w"))
    editing_claude.plan(writes={"global.md": "- rule\n"}, result=f"{queued.stem}: merged - ok\n")
    assert run("curate") == (0, "")
    assert (memory_repo / "global.md").read_text() == "- rule\n"
    assert notes.pending() == []
    notes.write("title: t\n---\nb", "user", Path("/w"))
    editing_claude.plan(exit_code=1)
    assert run("curate") == (1, "")


def test_curate_takes_no_flags(run: Runner):
    assert run("curate", "--weekly")[0] == 2
    assert run("curate", "--dry-run")[0] == 2


def test_skills_prints_the_drafts(run: Runner, memory_repo: Path, editing_claude: EditingClaude):
    editing_claude.plan(writes={"skill-ideas/jj-flow.md": "# idea\n"})
    assert run("skills") == (0, f"{memory_repo / 'skill-ideas' / 'jj-flow.md'}\n")
    editing_claude.plan(exit_code=1)
    assert run("skills") == (1, "")
