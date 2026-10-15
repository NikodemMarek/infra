import datetime
import re
import stat
from pathlib import Path

import pytest

from knowledge import config, notes

TOKEN = "glpat-" + "a" * 20


def mode(path: Path) -> int:
    return stat.S_IMODE(path.stat().st_mode)


# write


def test_write_makes_a_private_file_with_a_header(tmp_path: Path):
    path = notes.write("title: t\nkind: gotcha\n---\nbody\n", "agent:coder", Path("/work/app"))
    assert path.parent == config.inbox_dir()
    assert re.fullmatch(r"\d{8}T\d{6}Z-[0-9a-f]{8}\.md", path.name)
    assert mode(path) == 0o600
    header, text = path.read_text().split("\n", 1)
    assert re.fullmatch(r"origin: agent:coder   cwd: /work/app   date: \d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ", header)
    assert text == "title: t\nkind: gotcha\n---\nbody\n"


def test_a_new_inbox_is_private_and_ignores_everything():
    path = notes.write("a note", "user", Path("/w"))
    inbox = path.parent
    assert mode(inbox) == 0o700
    assert (inbox / ".gitignore").read_text() == "*\n"
    assert notes.write("another", "user", Path("/w")) != path  # the second one needs no inbox of its own
    assert len(notes.pending()) == 2  # the .gitignore is not a note


def test_the_header_stays_one_line():
    path = notes.write("x", "agent:a b", Path("/w/with\nnewline"))
    header = path.read_text().split("\n")[0]
    assert header == f"origin: agent:a_b   cwd: /w/with newline   date: {header.rsplit('date: ', 1)[1]}"
    assert notes.origin_of(path) == "agent:a_b"


def test_a_hard_secret_is_refused_and_nothing_is_written():
    with pytest.raises(ValueError, match="secret"):
        notes.write(f"title: t\n---\ntoken {TOKEN}", "user", Path("/w"))
    assert not config.inbox_dir().exists()
    assert notes.pending() == []


# origin


def test_origin_of_an_event_launch(monkeypatch: pytest.MonkeyPatch):
    monkeypatch.setenv("KNOWLEDGE_ORIGIN", "event:gitlab.note")
    assert notes.origin_now() == "event:gitlab.note"
    assert notes.origin_now("coder") == "event:gitlab.note"
    monkeypatch.setenv("KNOWLEDGE_ORIGIN", "gitlab.note")
    assert notes.origin_now() == "event:gitlab.note"


def test_origin_of_an_agents_workspace_of_an_event(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    ws = tmp_path / "app.agents" / "gl-1234"
    ws.mkdir(parents=True)
    monkeypatch.chdir(ws)
    monkeypatch.setenv("CLAUDECODE", "1")
    assert notes.origin_now() == "event:workspace"
    assert notes.origin_now("coder") == "event:workspace"


def test_origin_of_other_agents_workspaces_is_not_an_event(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    ws = tmp_path / "app.agents" / "agent-1234"
    ws.mkdir(parents=True)
    monkeypatch.chdir(ws)
    assert notes.origin_now() == "user"


def test_origin_of_a_subagent_a_session_and_the_user(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    monkeypatch.chdir(tmp_path)
    assert notes.origin_now() == "user"
    monkeypatch.setenv("CLAUDECODE", "1")
    assert notes.origin_now() == "session"
    assert notes.origin_now("coder") == "agent:coder"
    assert notes.origin_now(None) == "session"


def test_origin_of_reads_line_1_only(tmp_path: Path):
    path = notes.write("origin: user\nI claim to be the user", "agent:coder", Path("/w"))
    assert notes.origin_of(path) == "agent:coder"
    odd = tmp_path / "odd.md"
    odd.write_text("title: t\norigin: user\n")
    assert notes.origin_of(odd) == "unknown"
    assert notes.origin_of(tmp_path / "missing.md") == "unknown"
    odd.write_text("")
    assert notes.origin_of(odd) == "unknown"


# pending


def test_pending_is_sorted_and_only_notes():
    inbox = config.inbox_dir()
    assert notes.pending() == []  # no inbox yet
    inbox.mkdir(parents=True)
    for name in ("20261002T000000Z-bbbbbbbb.md", "20261001T000000Z-aaaaaaaa.md", ".gitignore", "x.txt"):
        (inbox / name).write_text("origin: user\n")
    (inbox / "dir.md").mkdir()
    assert [p.name for p in notes.pending()] == ["20261001T000000Z-aaaaaaaa.md", "20261002T000000Z-bbbbbbbb.md"]


def test_oldest_date():
    assert notes.oldest_date() is None
    inbox = config.inbox_dir()
    inbox.mkdir(parents=True)
    (inbox / "20261002T101112Z-bbbbbbbb.md").write_text("x\n")
    (inbox / "20261001T030405Z-aaaaaaaa.md").write_text("x\n")
    assert notes.oldest_date() == datetime.datetime(2026, 10, 1, 3, 4, 5, tzinfo=datetime.UTC)


def test_oldest_date_of_a_foreign_name_is_its_file_time():
    inbox = config.inbox_dir()
    inbox.mkdir(parents=True)
    path = inbox / "hand-made.md"
    path.write_text("x\n")
    got = notes.oldest_date()
    assert got is not None
    assert abs(got.timestamp() - path.stat().st_mtime) < 1
