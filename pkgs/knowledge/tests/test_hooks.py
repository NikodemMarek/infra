import io
import json
from pathlib import Path

import pytest

from knowledge import cli, config, context, hooks, notes

TOKEN = "glpat-" + "a" * 20
BLOCK = "```knowledge\ntitle: jj absorb\nkind: gotcha\nscope: topic/jj\nevidence: dotfiles\n---\nRule, why.\n```"
NOTE = "title: jj absorb\nkind: gotcha\nscope: topic/jj\nevidence: dotfiles\n---\nRule, why."


class Hook:
    def __init__(self, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]) -> None:
        self.monkeypatch = monkeypatch
        self.capsys = capsys

    def __call__(self, name: str, payload: object) -> tuple[int, dict | None]:
        """Run `knowledge hook <name>` with the payload (JSON, or text as it is) on stdin: the exit code and the JSON it printed."""
        stdin = payload if isinstance(payload, str) else json.dumps(payload)
        self.monkeypatch.setattr("sys.argv", ["knowledge", "hook", name])
        self.monkeypatch.setattr("sys.stdin", io.StringIO(stdin))
        code = 0
        try:
            cli.main()
        except SystemExit as e:
            code = int(e.code or 0)
        out = self.capsys.readouterr().out
        assert out == "" or out.count("\n") == 1
        return code, json.loads(out) if out else None

    def start(self, cwd: Path | str) -> dict | None:
        code, out = self("session-start", {"session_id": "s1", "cwd": str(cwd), "hook_event_name": "SessionStart"})
        assert code == 0
        return out

    def stop(self, **fields: object) -> dict | None:
        payload = {"session_id": "sess1", "cwd": "/home/u/app", "hook_event_name": "SubagentStop", "agent_id": "a1b2c3"}
        code, out = self("subagent-stop", {**payload, **fields})
        assert code == 0
        return out


@pytest.fixture
def hook(tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]) -> Hook:
    monkeypatch.chdir(tmp_path)  # not an event's `.agents/gl-*` workspace, whatever runs the tests
    return Hook(monkeypatch, capsys)


@pytest.fixture
def work(tmp_path: Path) -> Path:
    """A directory under the path rule of SCOPES.toml, whose scope file exists."""
    memory = config.memory_dir()
    (memory / "org").mkdir(parents=True)
    (memory / "SCOPES.toml").write_text(f'[[path]]\nprefix = "{tmp_path}/work"\nscopes = ["org/acme"]\n')
    (memory / "org" / "acme.md").write_text("- acme rule\n")
    return tmp_path / "work" / "app"


def queued() -> list[str]:
    """The text of each queued note without its header line."""
    return ["".join(p.read_text().splitlines(keepends=True)[1:]).rstrip("\n") for p in notes.pending()]


def old_note() -> None:
    inbox = config.inbox_dir()
    inbox.mkdir(parents=True)
    (inbox / "20200101T000000Z-aaaaaaaa.md").write_text("origin: user   cwd: /w   date: 2020-01-01T00:00:00Z\nold\n")


# session-start


def test_session_start_injects_the_scope_files(hook: Hook, work: Path):
    out = hook.start(work)
    assert out is not None
    assert set(out) == {"hookSpecificOutput"}
    got = out["hookSpecificOutput"]
    assert got["hookEventName"] == "SessionStart"
    assert "## org/acme.md\n- acme rule" in got["additionalContext"]


def test_session_start_without_files_or_stale_notes_prints_nothing(hook: Hook, work: Path, tmp_path: Path):
    assert hook.start(tmp_path / "elsewhere") is None
    notes.write("fresh", "user", Path("/w"))
    assert hook.start(tmp_path / "elsewhere") is None


def test_session_start_warns_of_stale_notes_in_any_directory(hook: Hook, tmp_path: Path):
    old_note()
    out = hook.start(tmp_path)
    assert out is not None
    assert set(out) == {"systemMessage"}
    assert "1 knowledge notes wait in" in out["systemMessage"]
    assert "systemctl --user status knowledge-curate" in out["systemMessage"]


def test_session_start_with_both(hook: Hook, work: Path):
    old_note()
    out = hook.start(work)
    assert out is not None
    assert set(out) == {"hookSpecificOutput", "systemMessage"}


def test_an_invalid_scopes_file_fails_soft(hook: Hook, work: Path, tmp_path: Path):
    (config.memory_dir() / "SCOPES.toml").write_text("this is = = not toml")
    old_note()
    out = hook.start(work)
    assert out is not None
    assert set(out) == {"systemMessage"}
    assert "SCOPES.toml" in out["systemMessage"]
    assert "knowledge notes wait" in out["systemMessage"]


def test_an_unexpected_error_in_the_scope_files_keeps_the_stale_line(hook: Hook, work: Path, monkeypatch: pytest.MonkeyPatch):
    def boom(cwd: Path) -> str | None:
        raise RuntimeError("disk on fire")

    monkeypatch.setattr(context, "session_context", boom)
    old_note()
    out = hook.start(work)
    assert out is not None
    assert set(out) == {"systemMessage"}
    assert "knowledge: disk on fire" in out["systemMessage"]
    assert "knowledge notes wait" in out["systemMessage"]


def test_session_start_without_a_cwd_uses_the_current_directory(hook: Hook, work: Path, monkeypatch: pytest.MonkeyPatch):
    work.mkdir(parents=True)
    monkeypatch.chdir(work)
    code, out = hook("session-start", {"session_id": "s1"})
    assert code == 0
    assert out is not None
    assert "acme rule" in out["hookSpecificOutput"]["additionalContext"]


def test_session_start_bad_input_prints_nothing_and_exits_0(hook: Hook):
    assert hook("session-start", "not json") == (0, None)
    assert hook("session-start", "[1]") == (0, None)
    assert hook("session-start", "") == (0, None)


def test_hooks_do_nothing_inside_a_curator_run(hook: Hook, work: Path, monkeypatch: pytest.MonkeyPatch):
    monkeypatch.setenv("KNOWLEDGE_CURATOR", "1")
    old_note()
    assert hook.start(work) is None
    assert hook.stop(last_assistant_message=BLOCK) is None
    assert len(notes.pending()) == 1  # the old one only


# subagent-stop


def test_subagent_stop_queues_the_blocks_as_notes(hook: Hook):
    out = hook.stop(last_assistant_message=f"Done.\n\n{BLOCK}\n", agent_type="coder")
    assert out == {"systemMessage": "knowledge: queued 1 note(s)"}
    assert queued() == [NOTE]
    (path,) = notes.pending()
    assert path.read_text().startswith("origin: agent:coder   cwd: /home/u/app   date: ")
    assert path.stat().st_mode & 0o777 == 0o600


def test_a_block_without_optional_fields(hook: Hook):
    hook.stop(last_assistant_message="```knowledge\ntitle: t\n---\nbody\n```")
    assert queued() == ["title: t\n---\nbody"]


def test_the_origin_is_the_agent_type_or_subagent_or_the_event(hook: Hook, monkeypatch: pytest.MonkeyPatch):
    hook.stop(last_assistant_message=BLOCK, agent_type="reviewer")
    hook.stop(last_assistant_message=BLOCK.replace("jj absorb", "other"))
    monkeypatch.setenv("KNOWLEDGE_ORIGIN", "event:gitlab.note")
    hook.stop(last_assistant_message=BLOCK.replace("jj absorb", "third"), agent_type="coder")
    assert sorted(notes.origin_of(p) for p in notes.pending()) == ["agent:reviewer", "agent:subagent", "event:gitlab.note"]


def test_several_blocks_and_the_same_one_twice(hook: Hook):
    other = BLOCK.replace("jj absorb", "second")
    out = hook.stop(last_assistant_message=f"{BLOCK}\ntext\n{other}\n{BLOCK}")
    assert out == {"systemMessage": "knowledge: queued 2 note(s)"}
    assert len(notes.pending()) == 2


def test_a_secret_is_refused_and_the_rest_queued(hook: Hook):
    secret = f"```knowledge\ntitle: leak\n---\ntoken {TOKEN}\n```"
    out = hook.stop(last_assistant_message=f"{secret}\n{BLOCK}")
    assert out == {"systemMessage": "knowledge: queued 1 note(s); refused 1 block(s) that look like secrets"}
    assert queued() == [NOTE]


def test_an_invalid_block_is_reported(hook: Hook):
    out = hook.stop(last_assistant_message="```knowledge\ntitle: no separator\nbody\n```")
    assert out == {"systemMessage": "knowledge: queued 0 note(s); skipped 1 invalid block(s) (need title: and ---)"}
    assert notes.pending() == []


def test_a_valid_and_an_invalid_block(hook: Hook):
    out = hook.stop(last_assistant_message=f"{BLOCK}\n```knowledge\n---\nno title\n```")
    assert out == {"systemMessage": "knowledge: queued 1 note(s); skipped 1 invalid block(s) (need title: and ---)"}
    assert queued() == [NOTE]


def test_no_blocks_or_a_continuing_subagent_print_nothing(hook: Hook):
    assert hook.stop(last_assistant_message="no knowledge here") is None
    assert hook.stop(last_assistant_message=BLOCK, stop_hook_active=True) is None
    assert hook.stop() is None
    assert notes.pending() == []


def test_subagent_stop_bad_input_says_so_and_exits_0(hook: Hook):
    code, out = hook("subagent-stop", "not json")
    assert code == 0
    assert out is not None
    assert out["systemMessage"].startswith("knowledge hook error: ")


def test_a_failure_to_queue_is_reported_not_raised(hook: Hook, monkeypatch: pytest.MonkeyPatch):
    def boom(text: str, origin: str, cwd: Path) -> Path:
        raise OSError("disk full")

    monkeypatch.setattr(notes, "write", boom)
    assert hook.stop(last_assistant_message=BLOCK) == {"systemMessage": "knowledge hook error: disk full"}


# the text of the subagent


def assistant(*texts: str) -> str:
    return json.dumps({"type": "assistant", "message": {"content": [{"type": "text", "text": t} for t in texts]}})


def test_the_text_comes_from_the_message_in_the_input():
    assert hooks.agent_text({"last_assistant_message": "hi", "agent_transcript_path": "/nowhere"}) == "hi"


def test_the_text_comes_from_the_transcript_in_the_input(tmp_path: Path):
    transcript = tmp_path / "t.jsonl"
    transcript.write_text("\n".join([assistant("first"), json.dumps({"type": "user"}), assistant("second", "third"), "{bad"]))
    assert hooks.agent_text({"agent_transcript_path": str(transcript)}) == "second\nthird"


def test_the_text_comes_from_the_subagents_directory(hook: Hook):
    path = config.claude_config_dir() / "projects" / "-home-u-app" / "sess1" / "subagents" / "agent-a1b2c3.jsonl"
    path.parent.mkdir(parents=True)
    path.write_text(assistant(BLOCK))
    hook.stop()  # session_id sess1, agent_id a1b2c3
    assert queued() == [NOTE]


def test_no_text_without_a_transcript(tmp_path: Path):
    assert hooks.agent_text({}) == ""
    assert hooks.agent_text({"agent_transcript_path": str(tmp_path / "missing.jsonl")}) == ""
    assert hooks.last_assistant_text(tmp_path / "missing.jsonl") == ""
