import datetime
from pathlib import Path

import pytest

from knowledge import config, context, notes
from knowledge.context import MarkerRule, PathRule, ProjectRow
from knowledge.data import ParseError

INDEX = """\
# Projects

| slug | status | repo |
|------|--------|------|
| dotfiles | active | `~/projects/dotfiles` |
| other | done | /nowhere/other |
| bare | active |  |
"""


@pytest.fixture
def memory(tmp_path: Path) -> Path:
    mem = tmp_path / "memory"
    mem.mkdir()
    return mem


def put(memory: Path, rel: str, text: str = "content\n") -> None:
    path = memory / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)


def mkrepo(path: Path, *names: str) -> Path:
    """A directory with a `.git` and the given marker files."""
    path.mkdir(parents=True)
    (path / ".git").mkdir()
    for name in names:
        (path / name).touch()
    return path


def scopes_toml(memory: Path, text: str) -> None:
    put(memory, "SCOPES.toml", text)


# the pieces


def test_projects_index_rows(memory: Path):
    assert context.projects_index(memory) == []  # no INDEX.md
    put(memory, "INDEX.md", INDEX)
    assert context.projects_index(memory) == [
        ProjectRow("dotfiles", "active", "~/projects/dotfiles"),
        ProjectRow("other", "done", "/nowhere/other"),
    ]


def test_project_for_cwd_takes_the_longest_repo(tmp_path: Path):
    rows = [ProjectRow("outer", "active", str(tmp_path / "a")), ProjectRow("inner", "active", str(tmp_path / "a" / "b"))]
    assert context.project_for_cwd(tmp_path / "a" / "b" / "c", rows) == rows[1]
    assert context.project_for_cwd(tmp_path / "a" / "x", rows) == rows[0]
    assert context.project_for_cwd(tmp_path / "z", rows) is None
    assert context.project_for_cwd(tmp_path / "ab", rows) is None  # a prefix of the name is not a parent


def test_normalise_cwd_maps_an_agents_workspace_to_its_repo():
    assert context.normalise_cwd(Path("/p/app.agents/ws-1/src")) == Path("/p/app/src")
    assert context.normalise_cwd(Path("/p/app.agents")) == Path("/p/app.agents")
    assert context.normalise_cwd(Path("/p/.agents/x")) == Path("/p/.agents/x")


def test_repo_root(tmp_path: Path):
    repo = mkrepo(tmp_path / "repo")
    (repo / "a" / "b").mkdir(parents=True)
    assert context.repo_root(repo / "a" / "b") == repo
    assert context.repo_root(tmp_path / "plain") == tmp_path / "plain"


def test_rules_ignore_budget_and_unknown_keys(memory: Path):
    assert context.load_rules(memory) == ([], [])  # no SCOPES.toml
    scopes_toml(
        memory,
        """
[[path]]
prefix = "~/projects/softnet"
scopes = ["org/softnet"]
note = "ignored"

[[marker]]
file = "flake.nix"
scopes = ["lang/nix", "topic/nix"]

[budget]
global = [40, 65536]
""",
    )
    assert context.load_rules(memory) == (
        [PathRule("~/projects/softnet", ("org/softnet",))],
        [MarkerRule("flake.nix", ("lang/nix", "topic/nix"))],
    )


def test_an_invalid_scopes_file_is_a_parse_error(memory: Path):
    scopes_toml(memory, "this is = = not toml")
    with pytest.raises(ParseError, match="SCOPES.toml"):
        context.load_rules(memory)
    scopes_toml(memory, "[[path]]\nscopes = ['org/x']\n")  # no prefix
    with pytest.raises(ParseError, match="SCOPES.toml"):
        context.load_rules(memory)


# which scope files


def test_a_path_rule(memory: Path, tmp_path: Path):
    scopes_toml(memory, f'[[path]]\nprefix = "{tmp_path}/work"\nscopes = ["topic/jj", "org/acme"]\n')
    assert context.scope_files(tmp_path / "work" / "app", memory) == ["org/acme.md", "topic/jj.md"]
    assert context.scope_files(tmp_path / "workshop", memory) == []


def test_a_marker_in_the_repo_root(memory: Path, tmp_path: Path):
    scopes_toml(memory, '[[marker]]\nfile = "flake.nix"\nscopes = ["lang/nix"]\n')
    repo = mkrepo(tmp_path / "repo", "flake.nix")
    (repo / "src" / "deep").mkdir(parents=True)
    assert context.scope_files(repo / "src" / "deep", memory) == ["lang/nix.md"]
    other = mkrepo(tmp_path / "other")
    assert context.scope_files(other, memory) == []


def test_an_agents_workspace_stands_for_its_repo(memory: Path, tmp_path: Path):
    scopes_toml(memory, '[[marker]]\nfile = "flake.nix"\nscopes = ["lang/nix"]\n')
    mkrepo(tmp_path / "proj", "flake.nix")
    (tmp_path / "proj.agents" / "ws-1" / "sub").mkdir(parents=True)  # a workspace has no .git of its own here
    assert context.scope_files(tmp_path / "proj.agents" / "ws-1" / "sub", memory) == ["lang/nix.md"]


def test_a_project_comes_from_the_index_and_goes_first(memory: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    put(memory, "INDEX.md", INDEX)
    scopes_toml(
        memory,
        '[[marker]]\nfile = "flake.nix"\nscopes = ["lang/nix", "org/softnet", "topic/jj"]\n',
    )
    home = tmp_path / "home"
    repo = mkrepo(home / "projects" / "dotfiles", "flake.nix")
    assert context.scope_files(repo / "src", memory) == [
        "projects/dotfiles/facts.md",
        "org/softnet.md",
        "lang/nix.md",
        "topic/jj.md",
    ]
    assert context.scope_files(home / "projects" / "dotfiles.agents" / "gl-1", memory)[0] == "projects/dotfiles/facts.md"


def test_unsafe_or_unknown_scopes_are_dropped(memory: Path, tmp_path: Path):
    scopes_toml(
        memory,
        f'[[path]]\nprefix = "{tmp_path}"\nscopes = ["global", "../../etc/passwd", "lang/Bad", "other/x", "lang/ok"]\n',
    )
    assert context.scope_files(tmp_path, memory) == ["lang/ok.md"]


# the injected text


def test_session_context_lists_the_existing_files(memory: Path, tmp_path: Path):
    scopes_toml(memory, f'[[path]]\nprefix = "{tmp_path}"\nscopes = ["org/acme", "lang/go", "topic/jj"]\n')
    put(memory, "org/acme.md", "- acme rule\n")
    put(memory, "topic/jj.md", "- jj rule\n")  # lang/go.md is missing: skipped
    put(memory, "global.md", "- global rule\n")
    out = context.session_context(tmp_path / "x")
    assert out is not None
    assert out.startswith(context.HEADER)
    assert out.index("## org/acme.md\n- acme rule") < out.index("## topic/jj.md\n- jj rule")
    assert "lang/go" not in out
    assert "global rule" not in out  # CLAUDE.md imports global.md
    assert "also relevant" not in out


def test_session_context_is_none_without_files(memory: Path, tmp_path: Path):
    assert context.session_context(tmp_path) is None  # no SCOPES.toml
    scopes_toml(memory, f'[[path]]\nprefix = "{tmp_path}"\nscopes = ["org/acme"]\n')
    assert context.session_context(tmp_path) is None  # no file
    put(memory, "org/acme.md", "  \n")
    assert context.session_context(tmp_path) is None  # an empty one


def test_session_context_without_a_memory_dir(tmp_path: Path):
    assert not config.memory_dir().exists()
    assert context.session_context(tmp_path) is None


def test_files_that_do_not_fit_are_listed(memory: Path, tmp_path: Path):
    scopes_toml(memory, f'[[path]]\nprefix = "{tmp_path}"\nscopes = ["org/a", "org/b", "org/c"]\n')
    for name in "abc":
        put(memory, f"org/{name}.md", name * 4000)
    out = context.session_context(tmp_path)
    assert out is not None
    assert len(out.encode()) <= context.MAX_BYTES
    assert "a" * 4000 in out and "b" * 4000 in out
    assert "c" * 4000 not in out
    assert f"also relevant: {memory / 'org' / 'c.md'}" in out.splitlines()


def test_the_also_relevant_lines_count_against_the_cap(memory: Path, tmp_path: Path):
    """Two files that fill the cap exactly leave no room for the line about the third: the second one is listed too."""
    scopes_toml(memory, f'[[path]]\nprefix = "{tmp_path}"\nscopes = ["org/a", "org/b", "org/c"]\n')
    put(memory, "org/a.md", "a" * 4000)
    # header + "\n" + 4013 + "\n" + (13 + n) + "\n" is the cap exactly: both files fit, but not the line about the third
    put(memory, "org/b.md", "b" * (context.MAX_BYTES - len(context.HEADER) - 4029))
    put(memory, "org/c.md", "c" * 100)
    out = context.session_context(tmp_path)
    assert out is not None
    assert len(out.encode()) <= context.MAX_BYTES
    assert "## org/a.md" in out
    assert "## org/b.md" not in out
    lines = out.splitlines()
    assert f"also relevant: {memory / 'org' / 'b.md'}" in lines
    assert f"also relevant: {memory / 'org' / 'c.md'}" in lines


# the stale inbox line


def old_note(name: str) -> Path:
    inbox = config.inbox_dir()
    inbox.mkdir(parents=True, exist_ok=True)
    path = inbox / name
    path.write_text("origin: user   cwd: /w   date: 2026-01-01T00:00:00Z\nnote\n")
    return path


def test_stale_inbox_line():
    assert context.stale_inbox_line() is None  # nothing waits
    old_note("20261001T080000Z-aaaaaaaa.md")
    old_note("20261001T090000Z-bbbbbbbb.md")
    assert context.stale_inbox_line(datetime.datetime(2026, 10, 2, 7, 59, tzinfo=datetime.UTC)) is None  # not a day yet
    assert context.stale_inbox_line(datetime.datetime(2026, 10, 2, 8, 1, tzinfo=datetime.UTC)) == (
        f"2 knowledge notes wait in {config.inbox_dir()} since 2026-10-01; "
        "check: systemctl --user status knowledge-curate"
    )


def test_a_fresh_note_is_not_stale():
    notes.write("fresh", "user", Path("/w"))
    assert context.stale_inbox_line() is None
