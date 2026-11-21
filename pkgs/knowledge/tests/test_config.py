from pathlib import Path

import pytest
from knowledge import config


def test_memory_dir_precedence(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    assert config.memory_dir() == tmp_path / "memory"
    monkeypatch.delenv("CLAUDE_MEMORY_DIR")
    assert config.memory_dir() == tmp_path / "claude" / "memory"
    monkeypatch.delenv("CLAUDE_CONFIG_DIR")
    assert (
        config.memory_dir()
        == tmp_path / "home" / ".local" / "share" / "claude" / "memory"
    )


def test_claude_config_dir(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    assert config.claude_config_dir() == tmp_path / "claude"
    monkeypatch.delenv("CLAUDE_CONFIG_DIR")
    assert (
        config.claude_config_dir() == tmp_path / "home" / ".local" / "share" / "claude"
    )


def test_an_empty_variable_counts_as_unset(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
):
    monkeypatch.setenv("CLAUDE_MEMORY_DIR", "")
    assert config.memory_dir() == tmp_path / "claude" / "memory"


def test_state_dir(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    assert config.state_dir() == tmp_path / "state" / "knowledge"
    monkeypatch.delenv("XDG_STATE_HOME")
    assert config.state_dir() == tmp_path / "home" / ".local" / "state" / "knowledge"


def test_runtime_dir(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    assert config.runtime_dir() == tmp_path / "run" / "knowledge"
    monkeypatch.delenv("XDG_RUNTIME_DIR")
    assert config.runtime_dir().parent.name.startswith("user-")
    assert config.runtime_dir().name == "knowledge"
    assert config.runtime_dir().parent.parent == Path("/tmp")


def test_paths_are_not_created_until_asked(tmp_path: Path):
    assert not config.state_dir().exists()
    d = config.ensure_private(config.state_dir())
    assert d.is_dir() and d.stat().st_mode & 0o077 == 0


def test_claude_bin(monkeypatch: pytest.MonkeyPatch):
    assert config.claude_bin() == "claude-bare"
    monkeypatch.setenv("KNOWLEDGE_CLAUDE", "/bin/fake-claude")
    assert config.claude_bin() == "/bin/fake-claude"


def test_flags(monkeypatch: pytest.MonkeyPatch):
    assert config.origin() is None
    assert not config.curator_active()
    monkeypatch.setenv("KNOWLEDGE_ORIGIN", "event:gitlab.note")
    monkeypatch.setenv("KNOWLEDGE_CURATOR", "1")
    assert config.origin() == "event:gitlab.note"
    assert config.curator_active()
    monkeypatch.setenv("KNOWLEDGE_CURATOR", "0")
    assert not config.curator_active()


def test_in_claude_code(monkeypatch: pytest.MonkeyPatch):
    assert not config.in_claude_code()
    monkeypatch.setenv("CLAUDECODE", "")
    assert not config.in_claude_code()
    monkeypatch.setenv("CLAUDECODE", "1")
    assert config.in_claude_code()
