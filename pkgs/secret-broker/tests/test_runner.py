import json
import os
import signal
import stat
import sys
import threading
import time
from pathlib import Path

import pytest

from secret_broker import runner
from secret_broker.model import RunResult
from secret_broker.runner import KEEP, action_env, child_dir, own_cgroup_procs, run_action, sweep

FIXED = {"PATH", "HOME", "TMPDIR", "LANG", "SSL_CERT_FILE", "NIX_SSL_CERT_FILE"}


def py(code: str) -> list[str]:
    return [sys.executable, "-c", code]


def run(
    code: str,
    secrets: dict[str, str] | None = None,
    timeout: int = 20,
    cancel: threading.Event | None = None,
) -> RunResult:
    return run_action(py(code), secrets or {}, timeout, cancel or threading.Event())


def gone(pid: int) -> bool:
    deadline = time.monotonic() + 3
    while time.monotonic() < deadline:
        try:
            state = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[0]
        except (FileNotFoundError, ProcessLookupError):
            return True
        if state == "Z":
            return True
        time.sleep(0.05)
    return False


BG = (
    "import subprocess,sys,time\n"
    "p=subprocess.Popen([sys.executable,'-c','import time; time.sleep(60)'])\n"
    "print(p.pid, flush=True)\n"
)


# child_dir


def test_child_dir_exists_private_empty_and_removed() -> None:
    with child_dir() as d:
        assert d.is_dir()
        assert stat.S_IMODE(d.stat().st_mode) == 0o700
        assert list(d.iterdir()) == []
        (d / "f").write_text("x")
        (d / "sub").mkdir()
        (d / "sub" / "g").write_text("y")
    assert not d.exists()


def test_child_dir_removed_on_exception() -> None:
    with pytest.raises(RuntimeError):
        with child_dir() as d:
            raise RuntimeError("boom")
    assert not d.exists()


def test_child_dirs_are_distinct() -> None:
    with child_dir() as a, child_dir() as b:
        assert a != b


# action_env


def test_action_env_exact(tmp_path: Path) -> None:
    assert action_env({"TOK": "v"}, tmp_path) == {
        "PATH": runner.FIXED_PATH,
        "HOME": str(tmp_path),
        "TMPDIR": str(tmp_path),
        "LANG": "C.UTF-8",
        "SSL_CERT_FILE": runner.CERTS,
        "NIX_SSL_CERT_FILE": runner.CERTS,
        "TOK": "v",
    }


def test_action_env_constants() -> None:
    assert runner.FIXED_PATH == "/run/current-system/sw/bin"
    assert runner.CERTS == "/etc/ssl/certs/ca-certificates.crt"


def test_action_env_without_secrets(tmp_path: Path) -> None:
    assert set(action_env({}, tmp_path)) == FIXED


# run_action


def test_run_echo() -> None:
    r = run("print('hello')")
    assert r.exit == 0
    assert r.stdout == b"hello\n"
    assert r.stderr == b""
    assert not (r.timed_out or r.cancelled or r.overflow)


def test_run_stderr_and_exit_code() -> None:
    r = run("import sys; sys.stderr.write('bad'); sys.exit(3)")
    assert r.exit == 3
    assert r.stderr == b"bad"
    assert r.stdout == b""


def test_run_env_is_fixed_plus_secrets_and_home_empty() -> None:
    code = (
        "import json,os\n"
        "print(json.dumps({'env': dict(os.environ), 'ls': os.listdir(os.environ['HOME']),"
        " 'cwd': os.getcwd(), 'isdir': os.path.isdir(os.environ['HOME'])}))\n"
    )
    r = run(code, {"TOK": "s3cret-value"})
    out = json.loads(r.stdout)
    env = out["env"]
    assert set(env) == FIXED | {"TOK"}
    assert env["TOK"] == "s3cret-value"
    assert env["PATH"] == runner.FIXED_PATH
    assert env["HOME"] == env["TMPDIR"]
    assert env["LANG"] == "C.UTF-8"
    assert env["SSL_CERT_FILE"] == env["NIX_SSL_CERT_FILE"] == runner.CERTS
    assert out["isdir"] is True
    assert out["ls"] == []
    assert os.path.realpath(out["cwd"]) == os.path.realpath(env["HOME"])


def test_run_does_not_inherit_parent_env(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("LEAK_ME", "1")
    out = json.loads(run("import json,os; print(json.dumps(sorted(os.environ)))").stdout)
    assert "LEAK_ME" not in out


def test_run_home_removed_afterwards() -> None:
    r = run("import os; print(os.environ['HOME'])")
    home = Path(r.stdout.decode().strip())
    assert str(home).startswith("/")
    assert not home.exists()


def test_run_home_removed_even_with_files_in_it() -> None:
    code = "import os; h=os.environ['HOME']; open(h+'/f','w').write('x'); os.mkdir(h+'/d'); print(h)"
    home = Path(run(code).stdout.decode().strip())
    assert not home.exists()


def test_secrets_never_in_argv() -> None:
    secret = "s3cret-value-xyz"
    r = run("print(open('/proc/self/cmdline','rb').read().decode(errors='replace'))", {"TOK": secret})
    assert secret.encode() not in r.stdout


def test_stdin_is_empty() -> None:
    r = run("import sys; print(repr(sys.stdin.read()))")
    assert r.stdout == b"''\n"


def test_child_is_own_session_leader() -> None:
    r = run("import os; print(os.getpgid(0) == os.getpid(), os.getpgid(0) != os.getpgid(os.getppid()))")
    assert r.stdout.startswith(b"True")


def test_sigterm_exit_is_143() -> None:
    r = run("import os,signal; os.kill(os.getpid(), signal.SIGTERM); import time; time.sleep(5)")
    assert r.exit == 143


def test_sigkill_exit_is_137() -> None:
    r = run("import os,signal; os.kill(os.getpid(), signal.SIGKILL)")
    assert r.exit == 137


def test_background_process_killed_after_leader_exits() -> None:
    r = run(BG)
    assert r.exit == 0
    assert gone(int(r.stdout))


def test_timeout_kills_and_returns_promptly() -> None:
    start = time.monotonic()
    r = run(BG + "time.sleep(60)", timeout=1)
    elapsed = time.monotonic() - start
    assert r.timed_out
    assert not r.cancelled
    assert elapsed < 2.0
    assert gone(int(r.stdout))


def test_cancel_kills_group() -> None:
    cancel = threading.Event()
    threading.Timer(1.0, cancel.set).start()
    start = time.monotonic()
    r = run(BG + "time.sleep(60)", timeout=30, cancel=cancel)
    assert time.monotonic() - start < 5
    assert r.cancelled
    assert not r.timed_out
    assert gone(int(r.stdout))


def test_cancel_already_set() -> None:
    cancel = threading.Event()
    cancel.set()
    start = time.monotonic()
    r = run("import time; time.sleep(60)", timeout=30, cancel=cancel)
    assert r.cancelled
    assert time.monotonic() - start < 5


def test_output_beyond_keep_sets_overflow() -> None:
    r = run(f"import sys; sys.stdout.buffer.write(b'a' * {KEEP + 5000}); sys.stdout.flush()")
    assert r.exit == 0
    assert r.overflow
    assert 0 < len(r.stdout) <= KEEP
    assert r.stdout == b"a" * len(r.stdout)


def test_stderr_beyond_keep_sets_overflow() -> None:
    r = run(f"import sys; sys.stderr.buffer.write(b'e' * {KEEP + 5000}); sys.stderr.flush()")
    assert r.exit == 0
    assert r.overflow
    assert len(r.stderr) <= KEEP


def test_output_of_exactly_keep_is_not_overflow() -> None:
    r = run(f"import sys; sys.stdout.buffer.write(b'a' * {KEEP}); sys.stdout.flush()")
    assert not r.overflow
    assert len(r.stdout) == KEEP


def test_large_stderr_and_stdout_do_not_deadlock() -> None:
    code = "import sys\nfor _ in range(200):\n    sys.stdout.write('o' * 4096); sys.stderr.write('e' * 4096)\n"
    r = run(code, timeout=20)
    assert r.exit == 0
    assert len(r.stdout) == len(r.stderr) == 200 * 4096


# sweep


def write_pids(path: Path, pids: list[int]) -> None:
    path.write_text("".join(f"{p}\n" for p in pids))


def test_sweep_only_own_pid(tmp_path: Path) -> None:
    procs = tmp_path / "procs"
    write_pids(procs, [100])
    kills: list[tuple[int, int]] = []
    sleeps: list[float] = []
    assert sweep(procs, 100, lambda p, s: kills.append((p, s)), sleeps.append) is True
    assert kills == []
    assert sleeps == []


def test_sweep_empty_file(tmp_path: Path) -> None:
    procs = tmp_path / "procs"
    procs.write_text("")
    kills: list[tuple[int, int]] = []
    assert sweep(procs, 100, lambda p, s: kills.append((p, s)), lambda _: None) is True
    assert kills == []


def test_sweep_kills_strays_until_empty(tmp_path: Path) -> None:
    procs = tmp_path / "procs"
    write_pids(procs, [100, 200, 300])
    kills: list[tuple[int, int]] = []
    sleeps: list[float] = []

    def sleep(t: float) -> None:
        sleeps.append(t)
        if len(sleeps) == 2:
            write_pids(procs, [100])

    assert sweep(procs, 100, lambda p, s: kills.append((p, s)), sleep) is True
    assert len(sleeps) == 2
    assert {p for p, _ in kills} == {200, 300}
    assert all(s == signal.SIGKILL for _, s in kills)


def test_sweep_never_empty_returns_false_after_ten_sleeps(tmp_path: Path) -> None:
    procs = tmp_path / "procs"
    write_pids(procs, [100, 200])
    kills: list[tuple[int, int]] = []
    sleeps: list[float] = []
    assert sweep(procs, 100, lambda p, s: kills.append((p, s)), sleeps.append) is False
    assert len(sleeps) == 10
    assert kills
    assert all(p == 200 for p, _ in kills)


def test_sweep_custom_rounds_and_wait(tmp_path: Path) -> None:
    procs = tmp_path / "procs"
    write_pids(procs, [100, 200])
    sleeps: list[float] = []
    assert sweep(procs, 100, lambda p, s: None, sleeps.append, rounds=3, wait=0.5) is False
    assert sleeps == [0.5, 0.5, 0.5]


def test_sweep_ignores_process_lookup_error(tmp_path: Path) -> None:
    procs = tmp_path / "procs"
    write_pids(procs, [100, 200, 300])
    killed: list[int] = []

    def kill(pid: int, sig: int) -> None:
        killed.append(pid)
        if pid == 200:
            raise ProcessLookupError

    def sleep(t: float) -> None:
        write_pids(procs, [100])

    assert sweep(procs, 100, kill, sleep) is True
    assert 200 in killed
    assert 300 in killed


# own_cgroup_procs


def test_own_cgroup_procs() -> None:
    lines = Path("/proc/self/cgroup").read_text().splitlines()
    unified = [ln for ln in lines if ln.startswith("0::")]
    if not unified:
        pytest.skip("no cgroup v2 line")
    expected = Path("/sys/fs/cgroup" + unified[0][3:] + "/cgroup.procs")
    assert own_cgroup_procs() == expected
