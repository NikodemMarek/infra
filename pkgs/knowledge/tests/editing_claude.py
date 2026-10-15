"""A stand-in for the `claude` of an editing call (curate, skills): a script that does what a plan file says.

The plan (JSON, set with `EditingClaude.plan`): `writes` {path: text} and `deletes` [path] (relative paths are relative to the
cwd of the call, the memory dir; absolute ones work too), `result` (the final text), `denials` (the permission_denials of the
result) and `exit` (a non-zero exit status: it fails before doing anything).
"""

import json
import sys
from pathlib import Path

import pytest

SCRIPT = """\
#!@PYTHON@
import json, os, sys
from pathlib import Path

plan = json.loads(Path(os.environ["FAKE_CLAUDE_PLAN"]).read_text())
stdin = sys.stdin.buffer.read().decode("utf-8", "replace")
Path(os.environ["FAKE_CLAUDE_LOG"]).write_text(json.dumps({"argv": sys.argv[1:], "stdin": stdin, "cwd": os.getcwd()}))
if plan.get("exit"):
    sys.stderr.write("boom\\n")
    sys.exit(plan["exit"])
for name, text in plan.get("writes", {}).items():
    path = Path(name)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)
for name in plan.get("deletes", []):
    Path(name).unlink()
print(json.dumps({"type": "result", "is_error": False, "result": plan.get("result", ""), "permission_denials": plan.get("denials", [])}))
"""


class EditingClaude:
    def __init__(self, plan_file: Path, log: Path) -> None:
        self.plan_file = plan_file
        self.log = log

    @classmethod
    def install(cls, tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> "EditingClaude":
        script = tmp_path / "bin" / "claude"
        script.parent.mkdir(exist_ok=True)
        script.write_text(SCRIPT.replace("@PYTHON@", sys.executable))
        script.chmod(0o755)
        fake = cls(tmp_path / "claude-plan.json", tmp_path / "claude-call.json")
        monkeypatch.setenv("KNOWLEDGE_CLAUDE", str(script))
        monkeypatch.setenv("FAKE_CLAUDE_PLAN", str(fake.plan_file))
        monkeypatch.setenv("FAKE_CLAUDE_LOG", str(fake.log))
        fake.plan()
        return fake

    def plan(
        self,
        writes: dict[str, str] | None = None,
        deletes: tuple[str, ...] = (),
        result: str = "",
        denials: tuple[str, ...] = (),
        exit_code: int = 0,
    ) -> None:
        doc = {"writes": writes or {}, "deletes": list(deletes), "result": result, "denials": list(denials), "exit": exit_code}
        self.plan_file.write_text(json.dumps(doc))

    @property
    def called(self) -> bool:
        return self.log.exists()

    def call(self) -> dict[str, object]:
        return json.loads(self.log.read_text())

    def stdin(self) -> str:
        return str(self.call()["stdin"])

    def argv(self) -> list[str]:
        args = self.call()["argv"]
        assert isinstance(args, list)
        return [str(a) for a in args]
