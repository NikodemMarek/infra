import logging
import os
import subprocess
from pathlib import Path

import pytest

from editing_claude import EditingClaude
from knowledge import config, prompts, skills


def sh(repo: Path, *args: str) -> str:
    proc = subprocess.run(
        ["git", "-c", "core.hooksPath=/dev/null", "-C", str(repo), *args],
        check=True,
        capture_output=True,
        text=True,
        stdin=subprocess.DEVNULL,
    )
    return proc.stdout


def skill(name: str, frontmatter: str) -> None:
    path = config.claude_config_dir() / "skills" / name / "SKILL.md"
    path.parent.mkdir(parents=True)
    path.write_text(frontmatter + "\n# body\nrules\n")


def old(path: Path) -> None:
    os.utime(path, (1_000_000_000, 1_000_000_000))


def test_frontmatter_forms():
    assert skills._frontmatter("---\nname: jj\ndescription: Use when using jj.\n---\nbody: not this\n") == {
        "name": "jj",
        "description": "Use when using jj.",
    }
    assert skills._frontmatter('---\nname: "q"\ndescription: >\n  folded\n  text\nother: x\n---\n') == {
        "name": "q",
        "description": "folded text",
        "other": "x",
    }
    assert skills._frontmatter("no frontmatter\nname: x\n") == {}
    assert skills._frontmatter("") == {}


def test_prints_the_drafts_it_created_or_changed_and_commits_nothing(
    memory_repo: Path, editing_claude: EditingClaude, capsys: pytest.CaptureFixture[str]
):
    ideas = memory_repo / "skill-ideas"
    ideas.mkdir()
    (ideas / "untouched.md").write_text("same\n")
    (ideas / "improved.md").write_text("v1\n")
    old(ideas / "untouched.md")
    old(ideas / "improved.md")
    skill("jj", "---\nname: jj\ndescription: Use when working with jj.\n---")
    skill("plain", "---\ndescription: Without a name.\n---")
    head = sh(memory_repo, "rev-parse", "HEAD")
    editing_claude.plan(writes={"skill-ideas/new.md": "---\nname: new\n---\nrules\n", "skill-ideas/improved.md": "v2\n"})
    assert skills.run() == 0
    assert capsys.readouterr().out.splitlines() == [str(ideas / "improved.md"), str(ideas / "new.md")]
    assert sh(memory_repo, "rev-parse", "HEAD") == head
    assert sh(memory_repo, "log", "--format=%s").splitlines() == ["init"]


def test_the_call(memory_repo: Path, editing_claude: EditingClaude):
    (memory_repo / "skill-ideas").mkdir()
    (memory_repo / "skill-ideas" / "draft.md").write_text("x\n")
    skill("jj", "---\nname: jj\ndescription: Use when working with jj.\n---")
    skill("plain", "---\ndescription: Without a name.\n---")
    assert skills.run() == 0
    stdin = editing_claude.stdin()
    assert "## jj\n\ndescription: Use when working with jj.\n\n# body\nrules\n" in stdin
    assert "## plain\n\ndescription: Without a name.\n\n# body\nrules\n" in stdin
    assert "name: jj" not in stdin  # the frontmatter is not repeated in the body
    assert "- skill-ideas/draft.md" in stdin
    args = editing_claude.argv()
    assert Path(str(editing_claude.call()["cwd"])).resolve() == memory_repo.resolve()
    assert args[args.index("--tools") + 1] == "Read,Grep,Glob,Edit,Write"
    assert args[args.index("--allowedTools") + 1] == f"Edit(/{memory_repo.resolve()}/skill-ideas/**)"
    assert args[args.index("--permission-mode") + 1] == "default"
    assert "skill ideas" in args[args.index("--append-system-prompt") + 1]


def test_a_long_skill_body_is_cut(memory_repo: Path, editing_claude: EditingClaude):
    skill("big", "---\nname: big\ndescription: Use when big.\n---\n" + "x" * (skills.MAX_BODY + 100))
    assert skills.run() == 0
    stdin = editing_claude.stdin()
    assert "x" * skills.MAX_BODY in stdin
    assert "x" * (skills.MAX_BODY + 1) not in stdin


def test_the_prompt_does_not_ask_to_read_a_deployed_skill():
    """The deployed skills are outside the cwd, where a read is denied: their bodies are in the message instead."""
    assert "SKILL.md first" not in prompts.read(skills.PROMPT)


def test_no_deployed_skills_and_no_drafts_yet(memory_repo: Path, editing_claude: EditingClaude):
    assert skills.run() == 0
    assert (memory_repo / "skill-ideas").is_dir()
    assert "(none)" in editing_claude.stdin()


def test_a_failed_call_exits_1(memory_repo: Path, editing_claude: EditingClaude, caplog: pytest.LogCaptureFixture):
    editing_claude.plan(writes={"skill-ideas/new.md": "x\n"}, denials=("Edit",))
    with caplog.at_level(logging.ERROR):
        assert skills.run() == 1
    assert "denied" in caplog.text


def test_no_memory_dir_exits_1(editing_claude: EditingClaude):
    assert skills.run() == 1
    assert not editing_claude.called
