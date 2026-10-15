import json
import sys
from pathlib import Path

import pytest

from fakes import FakeLlm
from knowledge.config import UsageError
from knowledge.llm import ALLOWED_TOOLS, DISALLOWED_TOOLS, PRIVATE_DENY, ClaudeCli, Llm, LlmError, build_argv

FAKE_CLAUDE = """\
#!@PYTHON@
import json, os, sys, time

mode = os.environ.get("FAKE_CLAUDE_MODE", "ok")
stdin = sys.stdin.read()
with open(os.environ["FAKE_CLAUDE_LOG"], "w") as f:
    json.dump(
        {
            "argv": sys.argv[1:],
            "stdin": stdin,
            "curator": os.environ.get("KNOWLEDGE_CURATOR"),
            "claudecode": os.environ.get("CLAUDECODE"),
            "cwd": os.getcwd(),
        },
        f,
    )
if mode == "sleep":
    time.sleep(30)
if mode == "exit1":
    sys.stderr.write("boom: no credentials\\n")
    sys.exit(1)
if mode == "notjson":
    print("this is not json")
    sys.exit(0)
if mode == "iserror":
    print(json.dumps({"is_error": True, "subtype": "error_during_execution"}))
    sys.exit(0)
if mode == "noresult":
    print(json.dumps({"is_error": False}))
    sys.exit(0)
if mode == "denied":
    print(json.dumps({"type": "result", "is_error": False, "result": "ok", "permission_denials": [{"tool_name": "Edit"}]}))
    sys.exit(0)
if not stdin.strip() or os.environ.get("KNOWLEDGE_CURATOR") != "1":
    sys.exit(3)
print(json.dumps({"type": "result", "is_error": False, "result": "```json\\n{}\\n```"}))
"""


@pytest.fixture
def fake_claude(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    """An executable that stands for `claude`; it logs what it was given to $FAKE_CLAUDE_LOG."""
    script = tmp_path / "bin" / "claude"
    script.parent.mkdir()
    script.write_text(FAKE_CLAUDE.replace("@PYTHON@", sys.executable))
    script.chmod(0o755)
    log = tmp_path / "claude-call.json"
    monkeypatch.setenv("KNOWLEDGE_CLAUDE", str(script))
    monkeypatch.setenv("FAKE_CLAUDE_LOG", str(log))
    return log


def call(log: Path) -> dict[str, object]:
    return json.loads(log.read_text())


# command line


def argv(**changes: object) -> list[str]:
    args: dict[str, object] = {
        "binary": "claude",
        "system": "SYSTEM",
        "add_dirs": [Path("/mem"), Path("/ws/skills")],
        "model": "sonnet",
        "effort": "medium",
    }
    args.update(changes)
    return build_argv(**args)  # type: ignore[arg-type]


def test_argv_has_every_flag_in_order():
    a = argv()
    assert a == [
        "claude",
        "-p",
        "--model",
        "sonnet",
        "--effort",
        "medium",
        "--output-format",
        "json",
        "--no-session-persistence",
        "--tools",
        ALLOWED_TOOLS,
        "--allowedTools",
        ALLOWED_TOOLS,
        "--disallowedTools",
        DISALLOWED_TOOLS,
        "--strict-mcp-config",
        "--settings",
        '{"disableAllHooks":true}',
        "--append-system-prompt",
        "SYSTEM",
        "--add-dir",
        "/mem",
        "--add-dir",
        "/ws/skills",
    ]


def test_argv_settings_parse_and_tools_are_read_only():
    a = argv()
    assert json.loads(a[a.index("--settings") + 1]) == {"disableAllHooks": True}
    assert a[a.index("--allowedTools") + 1] == "Read,Grep,Glob"
    assert a[a.index("--tools") + 1] == "Read,Grep,Glob"
    assert a[a.index("--tools") + 2].startswith("--")  # --tools is variadic: no positional right after it
    assert "--max-turns" not in a


def test_argv_has_no_positional_prompt():
    a = argv()
    flags_with_value = {
        "--model",
        "--effort",
        "--output-format",
        "--tools",
        "--allowedTools",
        "--disallowedTools",
        "--settings",
        "--append-system-prompt",
        "--add-dir",
    }
    rest = [x for i, x in enumerate(a[1:], 1) if not x.startswith("-") and a[i - 1] not in flags_with_value]
    assert rest == []


def test_argv_denies_the_private_notes():
    a = argv()
    denied = a[a.index("--disallowedTools") + 1].split(",")
    assert {"Read(./private/**)", "Grep(./private/**)", "Glob(./private/**)"} <= set(denied)
    assert denied == list(PRIVATE_DENY)
    assert "Read" not in denied  # only the private directory, not the tool
    assert a[a.index("--allowedTools") + 1] == "Read,Grep,Glob"


def test_argv_without_extra_dirs():
    a = argv(add_dirs=[])
    assert "--add-dir" not in a


def test_argv_without_tools_has_nothing_to_allow_or_deny():
    a = argv(tools="")
    assert a[a.index("--tools") + 1] == ""
    assert a[a.index("--tools") + 2] == "--strict-mcp-config"
    assert "--allowedTools" not in a
    assert "--disallowedTools" not in a
    assert "--effort" in a


def test_argv_with_edits_allows_only_edit_rules():
    a = argv(edits=["/mem/global.md", "/mem/topic/**"])
    assert a[a.index("--tools") + 1] == "Read,Grep,Glob,Edit,Write"
    assert a[a.index("--tools") + 2] == "--allowedTools"
    assert a[a.index("--allowedTools") + 1] == "Edit(//mem/global.md),Edit(//mem/topic/**)"  # no Write, no bare Read
    assert a[a.index("--disallowedTools") + 1] == DISALLOWED_TOOLS  # private/ stays denied
    assert a[a.index("--permission-mode") + 1] == "default"
    assert a.index("--permission-mode") < a.index("--strict-mcp-config")
    assert a[-4:] == ["--add-dir", "/mem", "--add-dir", "/ws/skills"]


def test_argv_with_edits_replaces_the_tools_argument():
    assert argv(tools="", edits=["/mem/x"]) == argv(edits=["/mem/x"])
    a = argv()
    assert "--permission-mode" not in a  # a call without edits is as before


def test_edits_must_be_absolute():
    with pytest.raises(ValueError, match="absolute"):
        argv(edits=["mem/global.md"])


# ClaudeCli


def test_run_returns_the_result(fake_claude: Path, memory_repo: Path):
    res = ClaudeCli().run("the system prompt", "the message", [memory_repo])
    assert res == "```json\n{}\n```"
    seen = call(fake_claude)
    assert seen["stdin"] == "the message"
    assert seen["curator"] == "1"
    assert Path(str(seen["cwd"])).resolve() == memory_repo.resolve()
    args = seen["argv"]
    assert isinstance(args, list)
    assert args[args.index("--append-system-prompt") + 1] == "the system prompt"
    assert args[args.index("--add-dir") + 1] == str(memory_repo)
    assert args[args.index("--model") + 1] == "sonnet"
    assert args[args.index("--effort") + 1] == "medium"


def test_tools_are_passed_through(fake_claude: Path, memory_repo: Path):
    ClaudeCli(tools="").run("s", "m", [])
    args = call(fake_claude)["argv"]
    assert isinstance(args, list)
    assert args[args.index("--tools") + 1] == ""
    assert "--allowedTools" not in args
    ClaudeCli().run("s", "m", [])
    args = call(fake_claude)["argv"]
    assert isinstance(args, list)
    assert args[args.index("--tools") + 1] == ALLOWED_TOOLS
    assert args[args.index("--allowedTools") + 1] == ALLOWED_TOOLS


def test_edits_are_passed_through(fake_claude: Path, memory_repo: Path):
    ClaudeCli(edits=["/mem/a.md"]).run("s", "m", [])
    args = call(fake_claude)["argv"]
    assert isinstance(args, list)
    assert args[args.index("--allowedTools") + 1] == "Edit(//mem/a.md)"
    assert args[args.index("--permission-mode") + 1] == "default"


def test_permission_denials_fail_a_call_with_edits(fake_claude: Path, memory_repo: Path, monkeypatch: pytest.MonkeyPatch):
    monkeypatch.setenv("FAKE_CLAUDE_MODE", "denied")
    with pytest.raises(LlmError, match="denied 1 tool call"):
        ClaudeCli(edits=["/mem/a.md"]).run("s", "m", [])
    assert ClaudeCli().run("s", "m", []) == "ok"  # a read-only call does not look at them


def test_the_child_does_not_see_claudecode(fake_claude: Path, memory_repo: Path, monkeypatch: pytest.MonkeyPatch):
    ClaudeCli().run("s", "m", [])
    assert call(fake_claude)["claudecode"] is None
    monkeypatch.setenv("CLAUDECODE", "1")  # a nested launch from inside Claude Code
    ClaudeCli().run("s", "m", [])
    assert call(fake_claude)["claudecode"] is None
    assert call(fake_claude)["curator"] == "1"


def test_model_and_effort_come_from_the_environment(
    fake_claude: Path, memory_repo: Path, monkeypatch: pytest.MonkeyPatch
):
    monkeypatch.setenv("KNOWLEDGE_MODEL", "opus-test")
    monkeypatch.setenv("KNOWLEDGE_EFFORT", "high")
    ClaudeCli().run("s", "m", [])
    args = call(fake_claude)["argv"]
    assert isinstance(args, list)
    assert args[args.index("--model") + 1] == "opus-test"
    assert args[args.index("--effort") + 1] == "high"


def test_a_failing_exit_is_an_error(fake_claude: Path, memory_repo: Path, monkeypatch: pytest.MonkeyPatch):
    monkeypatch.setenv("FAKE_CLAUDE_MODE", "exit1")
    with pytest.raises(LlmError, match="status 1: boom: no credentials"):
        ClaudeCli().run("s", "m", [])


def test_output_that_is_not_json_is_an_error(fake_claude: Path, memory_repo: Path, monkeypatch: pytest.MonkeyPatch):
    monkeypatch.setenv("FAKE_CLAUDE_MODE", "notjson")
    with pytest.raises(LlmError, match="did not print a JSON object"):
        ClaudeCli().run("s", "m", [])


def test_is_error_is_an_error(fake_claude: Path, memory_repo: Path, monkeypatch: pytest.MonkeyPatch):
    monkeypatch.setenv("FAKE_CLAUDE_MODE", "iserror")
    with pytest.raises(LlmError, match="error_during_execution"):
        ClaudeCli().run("s", "m", [])


def test_no_result_is_an_error(fake_claude: Path, memory_repo: Path, monkeypatch: pytest.MonkeyPatch):
    monkeypatch.setenv("FAKE_CLAUDE_MODE", "noresult")
    with pytest.raises(LlmError, match="no result"):
        ClaudeCli().run("s", "m", [])


def test_a_timeout_is_an_error(fake_claude: Path, memory_repo: Path, monkeypatch: pytest.MonkeyPatch):
    monkeypatch.setenv("FAKE_CLAUDE_MODE", "sleep")
    monkeypatch.setenv("KNOWLEDGE_TIMEOUT", "1")
    with pytest.raises(LlmError, match="timed out after 1s"):
        ClaudeCli().run("s", "m", [])


def test_a_missing_binary_is_an_error(memory_repo: Path, monkeypatch: pytest.MonkeyPatch, tmp_path: Path):
    monkeypatch.setenv("KNOWLEDGE_CLAUDE", str(tmp_path / "no-such-claude"))
    with pytest.raises(LlmError, match="cannot run claude"):
        ClaudeCli().run("s", "m", [])


def test_a_bad_timeout_is_a_usage_error(monkeypatch: pytest.MonkeyPatch):
    monkeypatch.setenv("KNOWLEDGE_TIMEOUT", "soon")
    with pytest.raises(UsageError):
        ClaudeCli()


# FakeLlm is an Llm


def test_fake_llm_stands_in_for_the_runner():
    fake = FakeLlm(lambda message: f"echo {message}")
    llm: Llm = fake
    assert llm.run("sys", "msg", [Path("/m")]) == "echo msg"
    assert fake.calls[0].add_dirs == (Path("/m"),)
    with pytest.raises(LlmError):
        FakeLlm(error=LlmError("down")).run("s", "m", [])
