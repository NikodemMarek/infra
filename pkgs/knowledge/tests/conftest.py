import os
import subprocess
from pathlib import Path

import pytest

from editing_claude import EditingClaude


@pytest.fixture(autouse=True)
def isolated_env(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """No test touches the real home, memory, state, lock directories, router or git identity."""
    for name in [n for n in os.environ if n.startswith("KNOWLEDGE_")]:
        monkeypatch.delenv(name)
    monkeypatch.delenv("CLAUDECODE", raising=False)  # set when the tests run inside Claude Code
    monkeypatch.setenv("HOME", str(tmp_path / "home"))
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "state"))
    monkeypatch.setenv("XDG_RUNTIME_DIR", str(tmp_path / "run"))
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", str(tmp_path / "claude"))
    monkeypatch.setenv("CLAUDE_MEMORY_DIR", str(tmp_path / "memory"))
    monkeypatch.setenv("KNOWLEDGE_SKILLS_DIR", str(tmp_path / "skills"))
    monkeypatch.setenv("EVENT_ROUTER_URL", "http://127.0.0.1:9")  # the discard port: nothing answers
    monkeypatch.setenv("GIT_CONFIG_GLOBAL", os.devnull)  # the user's hooks, signing and identity stay out
    monkeypatch.setenv("GIT_CONFIG_NOSYSTEM", "1")
    for who in ("AUTHOR", "COMMITTER"):
        monkeypatch.setenv(f"GIT_{who}_NAME", "Test")
        monkeypatch.setenv(f"GIT_{who}_EMAIL", "test@example.com")


@pytest.fixture
def editing_claude(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> EditingClaude:
    """KNOWLEDGE_CLAUDE is a script that does what `editing_claude.plan(...)` says (and logs the call)."""
    return EditingClaude.install(tmp_path, monkeypatch)


@pytest.fixture
def memory_repo(tmp_path: Path) -> Path:
    """A git repo at the memory dir with one commit (README.md)."""
    repo = tmp_path / "memory"
    repo.mkdir()
    (repo / "README.md").write_text("memory\n")
    for args in (
        ["init", "-q", "-b", "main"],
        ["add", "README.md"],
        ["commit", "-q", "--no-verify", "-m", "init"],
    ):
        subprocess.run(
            ["git", "-c", "core.hooksPath=/dev/null", "-C", str(repo), *args],
            check=True,
            capture_output=True,
            stdin=subprocess.DEVNULL,
        )
    return repo
