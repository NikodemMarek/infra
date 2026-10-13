from pathlib import Path

import pytest

from knowledge import scopes
from knowledge.data import ParseError
from knowledge.entry import EntryError
from knowledge.scopes import ScopesConfig
from knowledge.store import ProjectRow


@pytest.fixture
def cfg() -> ScopesConfig:
    return scopes.parse_scopes(scopes.DEFAULT_SCOPES_TOML)


@pytest.fixture
def work_cfg() -> ScopesConfig:
    """The defaults plus a path rule: everything under ~/projects/work gets org/work."""
    return scopes.parse_scopes(scopes.DEFAULT_SCOPES_TOML + '\n[[path]]\nprefix = "~/projects/work"\nscopes = ["org/work"]\n')


def mkrepo(path: Path, *names: str) -> Path:
    """A directory with a `.git` and the given marker files."""
    path.mkdir(parents=True)
    (path / ".git").mkdir()
    for name in names:
        (path / name).touch()
    return path


# loading


def test_defaults(cfg, tmp_path):
    assert scopes.load_scopes(tmp_path) == cfg  # no SCOPES.toml
    assert cfg.paths == ()
    assert len(cfg.markers) == 12
    assert scopes.MarkerRule("pom.xml", ("lang/java",)) in cfg.markers
    assert scopes.MarkerRule("build.gradle", ("lang/java",)) in cfg.markers
    assert cfg.total_budget == (300, 1048576)


def test_budget_for(cfg):
    assert cfg.budget_for("global") == (40, 65536)
    assert cfg.budget_for("lang/typescript") == (40, 65536)
    assert cfg.budget_for("topic/jj") == (30, 49152)
    assert cfg.budget_for("org/work") ==(80, 393216)
    assert cfg.budget_for("projects/dotfiles/facts") == (25, 49152)
    for bad in ("lang", "lang/Upper", "nothing/x", "projects/x"):
        with pytest.raises(EntryError):
            cfg.budget_for(bad)


def test_load_scopes_from_file(tmp_path):
    (tmp_path / "SCOPES.toml").write_text(
        """
[[path]]
prefix = "/work"
scopes = ["org/acme", "topic/jj"]

[budget]
topic = [5, 1000]
"""
    )
    cfg = scopes.load_scopes(tmp_path)
    assert cfg.paths == (scopes.PathRule("/work", ("org/acme", "topic/jj")),)
    assert cfg.markers == ()
    assert cfg.budget_for("topic/jj") == (5, 1000)
    assert cfg.budget_for("global") == (40, 65536)  # not in the file: the default
    assert cfg.total_budget == (300, 1048576)


@pytest.mark.parametrize(
    "text",
    [
        "this is = = not toml",
        "[[path]]\nprefix = '/x'\nscopes = ['lang/Bad']",
        "[[path]]\nprefix = '/x'\nscopes = []",
        "[[path]]\nscopes = ['global']",
        "[[path]]\nprefix = '/x'\nscopes = ['global']\nextra = 1",
        "[[marker]]\nfile = 'a/b'\nscopes = ['global']",
        "[budget]\nglobal = [1]",
        "[budget]\nglobal = [0, 10]",
        "[budget]\nglobal = ['a', 10]",
        "[budget]\nnonsense = [1, 2]",
        "unknown = 1",
        "path = 3",
    ],
)
def test_invalid_scopes_file(tmp_path, text):
    (tmp_path / "SCOPES.toml").write_text(text)
    with pytest.raises(ParseError, match="SCOPES.toml"):
        scopes.load_scopes(tmp_path)


def test_scopes_file_that_is_not_utf8(tmp_path):
    (tmp_path / "SCOPES.toml").write_bytes(b"\xff\xfe")
    with pytest.raises(ParseError):
        scopes.load_scopes(tmp_path)


# scopes_for_cwd


def test_work_path_with_tsconfig_and_a_project(work_cfg, tmp_path):
    repo = mkrepo(tmp_path / "home/projects/work/app-new", "tsconfig.json", "package.json")
    cwd = repo / "ui" / "src"
    cwd.mkdir(parents=True)
    (cwd / "Dockerfile").touch()  # a marker in the cwd counts too
    projects = [ProjectRow("app", "active", str(repo)), ProjectRow("ai", "active", "/nowhere/ai")]
    assert scopes.scopes_for_cwd(cwd, work_cfg, projects, ["app", "ai"]) == [
        "projects/app/facts",
        "org/work",
        "lang/javascript",
        "lang/typescript",
        "topic/docker",
        "global",
    ]


def test_project_without_a_store_slug_gets_no_facts(work_cfg, tmp_path):
    repo = mkrepo(tmp_path / "home/projects/work/app-new")
    projects = [ProjectRow("app", "active", str(repo))]
    assert scopes.scopes_for_cwd(repo, work_cfg, projects, []) == ["org/work", "global"]


def test_longest_project_path_wins(cfg, tmp_path):
    outer = mkrepo(tmp_path / "work/outer")
    inner = mkrepo(outer / "inner")
    projects = [ProjectRow("outer", "active", str(outer)), ProjectRow("inner", "active", str(inner))]
    assert scopes.scopes_for_cwd(inner / "x", cfg, projects, ["outer", "inner"])[0] == "projects/inner/facts"
    assert scopes.scopes_for_cwd(outer, cfg, projects, ["outer", "inner"])[0] == "projects/outer/facts"
    # a sibling with the same name prefix is not inside the project
    assert scopes.scopes_for_cwd(tmp_path / "work/outer-two", cfg, projects, ["outer", "inner"]) == ["global"]


def test_project_for_cwd_picks_the_longest_prefix(tmp_path):
    outer = mkrepo(tmp_path / "work/outer")
    inner = mkrepo(outer / "inner")
    rows = [ProjectRow("outer", "active", str(outer)), ProjectRow("inner", "active", str(inner))]
    assert scopes.project_for_cwd(inner / "x", rows) == rows[1]
    assert scopes.project_for_cwd(outer / "src", rows) == rows[0]
    assert scopes.project_for_cwd(tmp_path / "work/outer-two", rows) is None  # not inside the project
    assert scopes.project_for_cwd(outer, []) is None


def test_project_for_cwd_handles_agent_workspaces(tmp_path):
    rows = [ProjectRow("repo", "active", str(tmp_path / "p/repo"))]
    assert scopes.project_for_cwd(tmp_path / "p/repo.agents/gl-review-7/src", rows) == rows[0]
    assert scopes.project_for_cwd(tmp_path / "p/repo.agents", rows) is None  # no workspace named


def test_project_path_with_a_tilde(cfg, tmp_path):
    repo = mkrepo(tmp_path / "home/projects/ai", "flake.nix", ".jj")
    projects = [ProjectRow("ai", "active", "~/projects/ai")]
    assert scopes.scopes_for_cwd(repo, cfg, projects, ["ai"]) == [
        "projects/ai/facts",
        "lang/nix",
        "topic/jj",
        "global",
    ]


def test_agent_workspace_is_its_repo(cfg, tmp_path):
    repo = mkrepo(tmp_path / "p/repo", "pyproject.toml")
    (repo / "src").mkdir()
    projects = [ProjectRow("repo", "active", str(repo))]
    cwd = tmp_path / "p/repo.agents/gl-review-7/src"  # does not exist
    assert scopes.normalise_cwd(cwd) == repo / "src"
    assert scopes.scopes_for_cwd(cwd, cfg, projects, ["repo"]) == [
        "projects/repo/facts",
        "lang/python",
        "global",
    ]
    assert scopes.normalise_cwd(tmp_path / "p/repo.agents") == tmp_path / "p/repo.agents"
    assert scopes.normalise_cwd(tmp_path / "p/.agents/x") == tmp_path / "p/.agents/x"


def test_normalise_cwd_removes_dot_dot(tmp_path):
    assert scopes.normalise_cwd(tmp_path / "a" / ".." / "b") == tmp_path / "b"


def test_no_repo_root_uses_the_cwd(cfg, tmp_path):
    cwd = tmp_path / "loose"
    cwd.mkdir()
    (cwd / "package.json").touch()
    assert scopes.repo_root(cwd) == cwd
    assert scopes.scopes_for_cwd(cwd, cfg, [], []) == ["lang/javascript", "global"]
    assert scopes.scopes_for_cwd(tmp_path / "nowhere", cfg, [], []) == ["global"]


def test_repo_root_is_the_nearest_ancestor(tmp_path):
    outer = mkrepo(tmp_path / "outer")
    inner = tmp_path / "outer" / "inner"
    (inner / ".jj").mkdir(parents=True)
    assert scopes.repo_root(inner / "a" / "b") == inner
    assert scopes.repo_root(outer / "x") == outer


def test_markers_come_from_the_root_and_the_cwd_without_duplicates(cfg, tmp_path):
    repo = mkrepo(tmp_path / "r", "pom.xml", "build.gradle")
    cwd = repo / "mod"
    cwd.mkdir()
    (cwd / "pom.xml").touch()
    assert scopes.scopes_for_cwd(cwd, cfg, [], []) == ["lang/java", "global"]
