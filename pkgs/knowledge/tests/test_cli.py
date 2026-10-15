import json
from pathlib import Path

import pytest

from knowledge import cli


def test_paths_prints_one_json_line(tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]):
    monkeypatch.setattr("sys.argv", ["knowledge", "paths"])
    cli.main()
    out = capsys.readouterr().out
    assert out.count("\n") == 1
    got = json.loads(out)
    assert got["memory_dir"] == str(tmp_path / "memory")
    assert got["claude_config_dir"] == str(tmp_path / "claude")
    assert got["state_dir"] == str(tmp_path / "state" / "knowledge")
    assert got["runtime_dir"] == str(tmp_path / "run" / "knowledge")
    assert got["inbox"] == str(tmp_path / "state" / "knowledge" / "inbox")
    assert got["skills_dir"] == str(tmp_path / "skills")
    assert got["router_url"] == "http://127.0.0.1:9"
    assert got["claude"] == "claude-bare"


def test_no_command_is_a_usage_error(monkeypatch: pytest.MonkeyPatch):
    monkeypatch.setattr("sys.argv", ["knowledge"])
    with pytest.raises(SystemExit) as e:
        cli.main()
    assert e.value.code == 2
