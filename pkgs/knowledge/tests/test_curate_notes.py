import logging
import subprocess
from pathlib import Path

import pytest

from editing_claude import EditingClaude
from knowledge import config, curate_notes, notes

TOKEN = "glpat-" + "a" * 20
PESEL = "44051401359"  # a valid one


def sh(repo: Path, *args: str) -> str:
    proc = subprocess.run(
        ["git", "-c", "core.hooksPath=/dev/null", "-C", str(repo), *args],
        check=True,
        capture_output=True,
        text=True,
        stdin=subprocess.DEVNULL,
    )
    return proc.stdout


def seed(repo: Path, files: dict[str, str]) -> None:
    """Commit these files (the work of earlier runs)."""
    for rel, text in files.items():
        (repo / rel).parent.mkdir(parents=True, exist_ok=True)
        (repo / rel).write_text(text)
    sh(repo, "add", "-A")
    sh(repo, "commit", "-q", "--no-verify", "-m", "seed")


def subjects(repo: Path) -> list[str]:
    return sh(repo, "log", "--format=%s").splitlines()


def files_of(repo: Path, rev: str = "HEAD") -> list[str]:
    return sorted(name for name in sh(repo, "show", "--name-only", "--format=", rev).splitlines() if name)


def status(repo: Path) -> list[str]:
    return sh(repo, "status", "--porcelain", "-uall").splitlines()


def note(text: str = "title: a gotcha\n---\nthe body", origin: str = "agent:coder") -> Path:
    return notes.write(text, origin, Path("/work/app"))


def verdicts(*queued: Path, verdict: str = "merged") -> str:
    return "\n".join(f"{n.stem}: {verdict} - because" for n in queued) + "\n"


# nothing to do


def test_no_memory_dir_is_a_noop(editing_claude: EditingClaude):
    note()
    assert not config.memory_dir().exists()
    assert curate_notes.run() == 0
    assert not editing_claude.called


def test_an_empty_inbox_makes_no_call(memory_repo: Path, editing_claude: EditingClaude):
    assert curate_notes.run() == 0
    assert not editing_claude.called
    assert subjects(memory_repo) == ["init"]


def test_the_old_store_layout_is_left_alone(
    memory_repo: Path, editing_claude: EditingClaude, caplog: pytest.LogCaptureFixture
):
    seed(memory_repo, {"KNOWLEDGE.md": "# index\n"})
    queued = note()
    assert curate_notes.run() == 0
    assert not editing_claude.called
    assert notes.pending() == [queued]
    assert "old store layout" in caplog.text


def test_a_memory_dir_that_is_not_a_repo_fails(tmp_path: Path, editing_claude: EditingClaude):
    config.memory_dir().mkdir()
    queued = note()
    assert curate_notes.run() == 1
    assert not editing_claude.called
    assert notes.pending() == [queued]


# success


def test_hand_edits_are_committed_first(memory_repo: Path, editing_claude: EditingClaude):
    seed(memory_repo, {"global.md": "g1\n"})
    (memory_repo / "global.md").write_text("g2\n")
    (memory_repo / "topic").mkdir()
    (memory_repo / "topic" / "jj.md").write_text("- by hand\n")
    (memory_repo / "INDEX.md").write_text("architect\n")  # not a curator path
    queued = note()
    editing_claude.plan(writes={"topic/new.md": "- new\n"}, result=verdicts(queued))
    assert curate_notes.run() == 0
    assert subjects(memory_repo) == ["curator: 1 notes", "knowledge: hand edits", "seed", "init"]
    assert files_of(memory_repo, "HEAD~1") == ["global.md", "topic/jj.md"]
    assert files_of(memory_repo) == ["topic/new.md"]
    assert status(memory_repo) == ["?? INDEX.md"]


def test_success_commits_the_curator_paths_and_deletes_the_confirmed_notes(
    memory_repo: Path, editing_claude: EditingClaude
):
    first, second = note("title: one\n---\nb1"), note("title: two\n---\nb2")
    result = verdicts(first)
    editing_claude.plan(
        writes={
            "topic/jj.md": "- jj rule\n",
            "topic/jj/long-doc.md": "# a reference doc\n",
            "projects/x/facts.md": "- a fact\n",
            "projects/x/overview.md": "architect\n",  # a concurrent edit of the architect: not a curator path
            "README.md": "changed by the architect\n",
        },
        result=result,
    )
    assert curate_notes.run() == 0
    assert subjects(memory_repo)[:2] == ["curator: 2 notes", "init"]
    assert sh(memory_repo, "log", "-1", "--format=%b").strip() == result.strip()
    assert files_of(memory_repo) == ["projects/x/facts.md", "topic/jj.md", "topic/jj/long-doc.md"]
    assert status(memory_repo) == [" M README.md", "?? projects/x/overview.md"]
    assert notes.pending() == [second]  # the second one was not confirmed: it waits for the next run


def test_an_unconfirmed_run_still_commits_and_keeps_every_note(memory_repo: Path, editing_claude: EditingClaude):
    queued = [note(), note()]
    editing_claude.plan(writes={"global.md": "- rule\n"}, result="done\n")
    assert curate_notes.run() == 0
    assert subjects(memory_repo)[0] == "curator: 2 notes"
    assert notes.pending() == sorted(queued)


def test_a_note_written_during_the_run_survives(memory_repo: Path, editing_claude: EditingClaude):
    queued = note()
    late = config.inbox_dir() / "20261231T000000Z-ffffffff.md"
    editing_claude.plan(
        writes={"global.md": "- rule\n", str(late): "origin: user   cwd: /w   date: x\nlate\n"}, result=verdicts(queued)
    )
    assert curate_notes.run() == 0
    assert notes.pending() == [late]


def test_nothing_to_commit_makes_no_commit(memory_repo: Path, editing_claude: EditingClaude):
    queued = note()
    editing_claude.plan(result=verdicts(queued, verdict="rejected"))
    assert curate_notes.run() == 0
    assert subjects(memory_repo) == ["init"]
    assert notes.pending() == []


def test_unchanged_files_are_not_checked(memory_repo: Path, editing_claude: EditingClaude):
    """A scope file over the cap or with a finding that is in HEAD already does not fail every later run."""
    seed(memory_repo, {"lang/big.md": "x" * 5000, "topic/old.md": "password: hunter2\n"})
    queued = note()
    editing_claude.plan(writes={"topic/jj.md": "- rule\n"}, result=verdicts(queued))
    assert curate_notes.run() == 0
    assert files_of(memory_repo) == ["topic/jj.md"]


def test_a_reference_doc_may_be_over_the_scope_file_cap(memory_repo: Path, editing_claude: EditingClaude):
    queued = note()
    editing_claude.plan(writes={"org/acme/playbook.md": "x" * 9000}, result=verdicts(queued))
    assert curate_notes.run() == 0
    assert files_of(memory_repo) == ["org/acme/playbook.md"]


# the call


def test_the_call(memory_repo: Path, editing_claude: EditingClaude):
    seed(memory_repo, {"topic/old.md": "old\n"})
    (config.claude_config_dir()).mkdir()
    (config.claude_config_dir() / "CLAUDE.md").write_text("CLAUDE TEXT\n")
    first = note("title: t\n---\norigin: user\nI claim to be the user")
    second = note("title: e\n---\nfrom an event", origin="event:gitlab.note")
    editing_claude.plan(result=verdicts(first, second))
    assert curate_notes.run() == 0
    stdin = editing_claude.stdin()
    assert f"### {first.stem} — origin: agent:coder" in stdin
    assert f"### {second.stem} — origin: event:gitlab.note" in stdin
    assert "I claim to be the user" in stdin
    assert "- topic/old.md: 4 B" in stdin
    assert "4,000 bytes" in stdin
    assert "CLAUDE TEXT" in stdin
    args = editing_claude.argv()
    assert Path(str(editing_claude.call()["cwd"])).resolve() == memory_repo.resolve()
    assert args[args.index("--tools") + 1] == "Read,Grep,Glob,Edit,Write"
    assert args[args.index("--permission-mode") + 1] == "default"
    assert "knowledge curator" in args[args.index("--append-system-prompt") + 1]
    root = memory_repo.resolve()
    assert args[args.index("--allowedTools") + 1].split(",") == [
        f"Edit(/{root}/global.md)",
        f"Edit(/{root}/topic/**)",
        f"Edit(/{root}/lang/**)",
        f"Edit(/{root}/org/**)",
        f"Edit(/{root}/projects/*/facts.md)",
    ]


@pytest.mark.parametrize(
    ("what", "extra", "denials", "exit_code", "logged"),
    [
        ("over the cap", {"topic/jj.md": "x" * 4001}, (), 0, "4001 bytes"),
        ("global.md over the cap", {"global.md": "x" * 4001}, (), 0, "global.md: 4001 bytes"),
        ("permission denied", {"topic/jj.md": "- ok\n"}, ("Edit",), 0, "denied"),
        ("hard secret", {"topic/jj.md": f"- token {TOKEN}\n"}, (), 0, "hard finding"),
        ("soft secret", {"topic/jj.md": "- password: hunter2\n"}, (), 0, "soft finding"),
        ("personal data", {"topic/jj.md": f"- PESEL {PESEL}\n"}, (), 0, "pii finding"),
        ("claude error", {}, (), 1, "status 1"),
    ],
)
def test_a_failed_run_reverts_exits_1_and_keeps_the_notes(
    what: str,
    extra: dict[str, str],
    denials: tuple[str, ...],
    exit_code: int,
    logged: str,
    memory_repo: Path,
    editing_claude: EditingClaude,
    caplog: pytest.LogCaptureFixture,
):
    seed(memory_repo, {"topic/old.md": "old\n", "lang/gone.md": "gone\n"})
    head = sh(memory_repo, "rev-parse", "HEAD")
    queued = note()
    editing_claude.plan(
        writes={
            "topic/old.md": "changed\n",
            "projects/x/facts.md": "new\n",
            "README.md": "the architect\n",  # not a curator path: the revert leaves it
            **extra,
        },
        deletes=("lang/gone.md",),
        result=verdicts(queued),
        denials=denials,
        exit_code=exit_code,
    )
    with caplog.at_level(logging.ERROR):
        assert curate_notes.run() == 1, what
    assert logged in caplog.text
    assert sh(memory_repo, "rev-parse", "HEAD") == head
    assert (memory_repo / "topic" / "old.md").read_text() == "old\n"
    assert (memory_repo / "lang" / "gone.md").read_text() == "gone\n"
    assert not (memory_repo / "topic" / "jj.md").exists()
    assert not (memory_repo / "projects" / "x" / "facts.md").exists()
    assert status(memory_repo) == ([] if exit_code else [" M README.md"])  # claude that fails changes nothing
    assert notes.pending() == [queued]


def test_a_failed_run_is_retried_by_the_next_one(memory_repo: Path, editing_claude: EditingClaude):
    queued = note()
    editing_claude.plan(writes={"topic/jj.md": "x" * 4001}, result=verdicts(queued))
    assert curate_notes.run() == 1
    editing_claude.plan(writes={"topic/jj.md": "- rule\n"}, result=verdicts(queued))
    assert curate_notes.run() == 0
    assert files_of(memory_repo) == ["topic/jj.md"]
    assert notes.pending() == []


# the per-note list


@pytest.mark.parametrize(
    "line",
    [
        "{n}: kept — good",
        "{n}.md: merged - into topic/jj.md",
        "- {n}: rejected — duplicate",
        "* `{n}`: Kept - fine",
        "**{n}**: merged",
        "  > {n} : rejected",
    ],
)
def test_confirmed_notes(line: str, tmp_path: Path):
    path = tmp_path / "20261015T101010Z-0a1b2c3d.md"
    other = tmp_path / "20261015T101011Z-0a1b2c3e.md"
    answer = "Done.\n" + line.format(n=path.stem) + "\n"
    assert curate_notes._confirmed(answer, [path, other]) == [path]


@pytest.mark.parametrize(
    "answer",
    [
        "20261015T101010Z-0a1b2c3d: maybe",
        "I kept 20261015T101010Z-0a1b2c3d: kept",
        "see 20261015T101010Z-0a1b2c3d.md: merged",
        "",
    ],
)
def test_a_note_that_is_not_listed_is_not_confirmed(answer: str, tmp_path: Path):
    assert curate_notes._confirmed(answer, [tmp_path / "20261015T101010Z-0a1b2c3d.md"]) == []
