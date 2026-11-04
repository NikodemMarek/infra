import json
import sys
from pathlib import Path

import pytest

from secret_broker.export import ExportError, run_export

FAKE = """#!{py}
import json, os, sys
args = sys.argv[1:]
spec = open("secretspec.toml").read() if os.path.exists("secretspec.toml") else None
rec = {{
    "argv": args,
    "cwd": os.getcwd(),
    "home": os.environ.get("HOME"),
    "tmpdir": os.environ.get("TMPDIR"),
    "session": os.environ.get("BW_SESSION"),
    "spec": spec,
}}
with open(os.environ["FAKE_LOG"], "a") as f:
    f.write(json.dumps(rec) + "\\n")
p = args[2]
if p == os.environ.get("FAKE_FAIL"):
    sys.stderr.write("nope\\n")
    sys.exit(3)
if p == os.environ.get("FAKE_BAD"):
    print(os.environ["FAKE_BAD_OUT"])
    sys.exit(0)
print(json.dumps({{"KEY": p + "-val", "OTHER": "ünï"}}, ensure_ascii=False))
"""

SPEC = '[project]\nname = "x"\n'


class Fake:
    def __init__(self, tmp: Path) -> None:
        self.bin = tmp / "secretspec"
        self.bin.write_text(FAKE.format(py=sys.executable), encoding="utf-8")
        self.bin.chmod(0o755)
        self.log = tmp / "log.jsonl"
        self.env = {"FAKE_LOG": str(self.log), "PYTHONIOENCODING": "utf-8"}

    def calls(self) -> list[dict[str, str | list[str]]]:
        if not self.log.exists():
            return []
        return [json.loads(ln) for ln in self.log.read_text().splitlines()]

    def profiles(self) -> list[str]:
        out: list[str] = []
        for c in self.calls():
            argv = c["argv"]
            assert isinstance(argv, list)
            out.append(argv[2])
        return out


@pytest.fixture
def fake(tmp_path: Path) -> Fake:
    return Fake(tmp_path)


def test_two_profiles(fake: Fake) -> None:
    out = run_export(str(fake.bin), SPEC, ["a", "b"], "sess-1", fake.env)
    assert out == {
        "a": {"KEY": "a-val", "OTHER": "ünï"},
        "b": {"KEY": "b-val", "OTHER": "ünï"},
    }


def test_argv_exact_and_session_only_in_env(fake: Fake) -> None:
    run_export(str(fake.bin), SPEC, ["a", "b"], "sess-secret-1", fake.env)
    calls = fake.calls()
    assert [c["argv"] for c in calls] == [
        ["export", "--profile", "a", "--format", "json"],
        ["export", "--profile", "b", "--format", "json"],
    ]
    for c in calls:
        assert c["session"] == "sess-secret-1"
        argv = c["argv"]
        assert isinstance(argv, list)
        assert all("sess-secret-1" not in a for a in argv)


def test_spec_read_from_cwd_which_is_home_and_tmpdir(fake: Fake) -> None:
    spec = '[project]\nname = "ü"\n# marker 42\n'
    run_export(str(fake.bin), spec, ["a", "b"], "s", fake.env)
    calls = fake.calls()
    for c in calls:
        assert c["spec"] == spec
        assert c["cwd"] == c["home"] == c["tmpdir"]


def test_fresh_dir_removed_afterwards(fake: Fake) -> None:
    run_export(str(fake.bin), SPEC, ["a"], "s", fake.env)
    assert not Path(str(fake.calls()[0]["cwd"])).exists()


def test_dir_removed_on_failure(fake: Fake) -> None:
    with pytest.raises(ExportError):
        run_export(str(fake.bin), SPEC, ["a"], "s", {**fake.env, "FAKE_FAIL": "a"})
    assert not Path(str(fake.calls()[0]["cwd"])).exists()


def test_nonzero_exit(fake: Fake) -> None:
    with pytest.raises(ExportError) as e:
        run_export(str(fake.bin), SPEC, ["a"], "s", {**fake.env, "FAKE_FAIL": "a"})
    assert str(e.value) == "secretspec export failed for a (exit 3)"


def test_stops_at_first_failing_profile(fake: Fake) -> None:
    with pytest.raises(ExportError) as e:
        run_export(str(fake.bin), SPEC, ["a", "bad", "c"], "s", {**fake.env, "FAKE_FAIL": "bad"})
    assert str(e.value) == "secretspec export failed for bad (exit 3)"
    assert fake.profiles() == ["a", "bad"]


@pytest.mark.parametrize(
    "out", ["not json", "[1, 2]", '{"a": 1}', '{"a": null}', '{"a": {"b": "c"}}', '"str"', ""]
)
def test_invalid_output(fake: Fake, out: str) -> None:
    env = {**fake.env, "FAKE_BAD": "a", "FAKE_BAD_OUT": out}
    with pytest.raises(ExportError) as e:
        run_export(str(fake.bin), SPEC, ["a"], "s", env)
    assert str(e.value) == "secretspec export returned invalid JSON for a"


def test_empty_object_is_valid(fake: Fake) -> None:
    env = {**fake.env, "FAKE_BAD": "a", "FAKE_BAD_OUT": "{}"}
    assert run_export(str(fake.bin), SPEC, ["a"], "s", env) == {"a": {}}
