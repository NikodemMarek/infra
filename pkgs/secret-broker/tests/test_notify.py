import json
import sys
import time
from pathlib import Path

import pytest

from secret_broker.notify import notify
from secret_broker.runner import FIXED_PATH

ENV_VAR = "SECRET_BROKER_EVENT_PUB"
# variables the Python interpreter adds to its own environment (locale coercion)
INTERPRETER_VARS = {"LC_CTYPE"}


def fake_pub(tmp_path: Path, body: str = "") -> Path:
    """Python executable recording argv, stdin, cwd and env as files in tmp_path/rec.

    The shebang is the absolute interpreter, so it works with the fixed PATH
    (which does not exist in the build sandbox).
    """
    rec = tmp_path / "rec"
    rec.mkdir(exist_ok=True)
    script = tmp_path / "event-pub"
    script.write_text(
        f"#!{sys.executable}\n"
        "import json, os, sys, time\n"
        f"rec = {str(rec)!r}\n"
        'with open(rec + "/argv", "w") as f:\n'
        "    json.dump(sys.argv[1:], f)\n"
        'with open(rec + "/stdin", "w") as f:\n'
        "    f.write(sys.stdin.read())\n"
        'with open(rec + "/cwd", "w") as f:\n'
        "    f.write(os.getcwd())\n"
        'with open(rec + "/env", "w") as f:\n'
        "    json.dump(dict(os.environ), f)\n"
        f"{body}\n"
    )
    script.chmod(0o755)
    return script


def test_runs_event_pub_with_event(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv(ENV_VAR, str(fake_pub(tmp_path)))
    notify("Secret request")
    rec = tmp_path / "rec"
    assert json.loads((rec / "argv").read_text()) == ["secret.request"]
    stdin = (rec / "stdin").read_text()
    assert json.loads(stdin) == {
        "project": "secret-broker",
        "title": "Secret request",
        "timeout": 300,
    }
    assert stdin.strip() == (
        '{"project":"secret-broker","title":"Secret request","timeout":300}'
    )
    assert (rec / "cwd").read_text() == "/"


def test_env_is_only_path_and_home(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv(ENV_VAR, str(fake_pub(tmp_path)))
    monkeypatch.setenv("SECRET_LEAK", "x")
    notify("t")
    env = json.loads((tmp_path / "rec" / "env").read_text())
    for k in INTERPRETER_VARS:
        env.pop(k, None)
    assert env == {"PATH": FIXED_PATH, "HOME": "/var/empty"}


def test_title_is_json_encoded(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv(ENV_VAR, str(fake_pub(tmp_path)))
    notify('a "quoted"\nżółć')
    stdin = (tmp_path / "rec" / "stdin").read_text()
    assert json.loads(stdin)["title"] == 'a "quoted"\nżółć'


def test_unset_does_nothing(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    fake_pub(tmp_path)
    monkeypatch.delenv(ENV_VAR, raising=False)
    assert notify("t") is None
    assert not (tmp_path / "rec" / "argv").exists()


def test_empty_does_nothing(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    fake_pub(tmp_path)
    monkeypatch.setenv(ENV_VAR, "")
    assert notify("t") is None
    assert not (tmp_path / "rec" / "argv").exists()


def test_nonzero_exit_is_swallowed(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv(ENV_VAR, str(fake_pub(tmp_path, "sys.exit(1)")))
    assert notify("t") is None
    assert (tmp_path / "rec" / "argv").exists()


def test_missing_path_is_swallowed(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv(ENV_VAR, str(tmp_path / "nope"))
    assert notify("t") is None


def test_not_executable_is_swallowed(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    script = fake_pub(tmp_path)
    script.chmod(0o644)
    monkeypatch.setenv(ENV_VAR, str(script))
    assert notify("t") is None
    assert not (tmp_path / "rec" / "argv").exists()


def test_hang_times_out_quickly(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv(ENV_VAR, str(fake_pub(tmp_path, "time.sleep(10)")))
    start = time.monotonic()
    assert notify("t") is None
    assert time.monotonic() - start < 4
