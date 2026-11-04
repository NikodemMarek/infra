import json
import sys
import time
from pathlib import Path

import pytest

from secret_broker.bw import BwError, check_server, load, status, sync, unlocked

FAKE_BW = """#!{py}
import json, os, sys, time
args = sys.argv[1:]
home = os.environ.get("HOME", "")
isdir = os.path.isdir(home)
rec = {{
    "argv": args,
    "has_password": "BW_PASSWORD" in os.environ,
    "has_session": "BW_SESSION" in os.environ,
    "session": os.environ.get("BW_SESSION"),
    "home": home,
    "tmpdir": os.environ.get("TMPDIR"),
    "home_exists": isdir,
    "home_listing": sorted(os.listdir(home)) if isdir else None,
}}
with open(os.environ["FAKE_LOG"], "a") as f:
    f.write(json.dumps(rec) + "\\n")
cmd = args[0] if args else ""
if cmd == "status":
    default = json.dumps({{"serverUrl": None, "lastSync": "2024-01-01T00:00:00.000Z", "status": "locked"}})
    print(os.environ.get("FAKE_STATUS", default))
elif cmd == "unlock":
    if os.environ.get("FAKE_UNLOCK", "") == "empty":
        sys.exit(0)
    if os.environ.get("BW_PASSWORD") != os.environ.get("FAKE_PASSWORD"):
        sys.stderr.write("Invalid master password.\\n")
        sys.exit(1)
    sys.stdout.write("sess-token-xyz")
elif cmd == "sync":
    mode = os.environ.get("FAKE_SYNC", "")
    if mode == "fail":
        sys.stderr.write("boom\\n")
        sys.exit(1)
    if mode == "hang":
        time.sleep(30)
    print("Syncing complete.")
elif cmd == "list":
    if os.environ.get("BW_SESSION") != "sess-token-xyz":
        sys.exit(1)
    items = json.load(open(os.environ["FAKE_ITEMS"]))
    print(json.dumps(items.get(args[3], [])))
elif cmd == "lock":
    print("Your vault is locked.")
else:
    sys.exit(2)
"""

PASSWORD = "master-pw-123"
SESSION = "sess-token-xyz"

type Item = dict[str, object]


class Fake:
    def __init__(self, tmp: Path) -> None:
        self.bin = tmp / "bw"
        self.bin.write_text(FAKE_BW.format(py=sys.executable))
        self.bin.chmod(0o755)
        self.log = tmp / "log.jsonl"
        self.items = tmp / "items.json"
        self.items.write_text("{}")
        self.env = {
            "FAKE_LOG": str(self.log),
            "FAKE_ITEMS": str(self.items),
            "FAKE_PASSWORD": PASSWORD,
        }

    def set_items(self, items: dict[str, list[Item]]) -> None:
        self.items.write_text(json.dumps(items))

    def calls(self) -> list[dict[str, str | bool | list[str] | None]]:
        if not self.log.exists():
            return []
        return [json.loads(ln) for ln in self.log.read_text().splitlines()]

    def argvs(self) -> list[list[str]]:
        out: list[list[str]] = []
        for c in self.calls():
            argv = c["argv"]
            assert isinstance(argv, list)
            out.append(argv)
        return out


@pytest.fixture
def fake(tmp_path: Path) -> Fake:
    return Fake(tmp_path)


def login(name: str, pw: str) -> Item:
    return {"name": name, "type": 1, "login": {"password": pw}}


def test_status(fake: Fake) -> None:
    st = status(str(fake.bin), fake.env)
    assert st["status"] == "locked"
    assert st["serverUrl"] is None
    assert fake.argvs() == [["status"]]


def test_status_gets_fresh_home(fake: Fake) -> None:
    status(str(fake.bin), fake.env)
    c = fake.calls()[0]
    assert c["home_exists"] is True
    assert not Path(str(c["home"])).exists()


def test_check_server_ok() -> None:
    check_server({"serverUrl": None}, None)
    check_server({"serverUrl": "https://v.example"}, "https://v.example")


def test_check_server_mismatch_message() -> None:
    with pytest.raises(BwError) as e:
        check_server({"serverUrl": "https://a.example"}, "https://b.example")
    assert str(e.value) == "bw server is https://a.example, expected https://b.example"


def test_check_server_default_vs_expected() -> None:
    with pytest.raises(BwError):
        check_server({"serverUrl": None}, "https://b.example")


def test_check_server_set_vs_default_expected() -> None:
    with pytest.raises(BwError):
        check_server({"serverUrl": "https://a.example"}, None)


def test_call_order_and_exact_argv(fake: Fake) -> None:
    fake.set_items({"a": [login("a", "va")], "b": [login("b", "vb")]})
    with unlocked(str(fake.bin), PASSWORD, fake.env) as session:
        assert session == SESSION
        assert sync(str(fake.bin), session, fake.env) is None
        values, errors = load(str(fake.bin), session, ["a", "b"], fake.env)
    assert values == {"a": "va", "b": "vb"}
    assert errors == {}
    assert fake.argvs() == [
        ["unlock", "--passwordenv", "BW_PASSWORD", "--raw"],
        ["sync"],
        ["list", "items", "--search", "a"],
        ["list", "items", "--search", "b"],
        ["lock"],
    ]


def test_password_and_session_never_in_argv(fake: Fake) -> None:
    fake.set_items({"a": [login("a", "va")]})
    with unlocked(str(fake.bin), PASSWORD, fake.env) as session:
        sync(str(fake.bin), session, fake.env)
        load(str(fake.bin), session, ["a"], fake.env)
    for argv in fake.argvs():
        assert all(PASSWORD not in a and SESSION not in a for a in argv)


def test_password_only_in_unlock_env_session_in_the_rest(fake: Fake) -> None:
    fake.set_items({"a": [login("a", "va")]})
    with unlocked(str(fake.bin), PASSWORD, fake.env) as session:
        sync(str(fake.bin), session, fake.env)
        load(str(fake.bin), session, ["a"], fake.env)
    by_cmd = {c["argv"][0]: c for c in fake.calls()}  # type: ignore[index]
    assert by_cmd["unlock"]["has_password"] is True
    assert by_cmd["unlock"]["has_session"] is False
    for cmd in ("sync", "list"):
        assert by_cmd[cmd]["has_password"] is False
        assert by_cmd[cmd]["session"] == SESSION
    assert by_cmd["lock"]["has_password"] is False


def test_each_child_gets_fresh_existing_empty_home(fake: Fake) -> None:
    fake.set_items({"a": [login("a", "va")], "b": [login("b", "vb")]})
    with unlocked(str(fake.bin), PASSWORD, fake.env) as session:
        sync(str(fake.bin), session, fake.env)
        load(str(fake.bin), session, ["a", "b"], fake.env)
    calls = fake.calls()
    homes = [str(c["home"]) for c in calls]
    assert len(calls) == 5
    assert len(set(homes)) == 5
    for c in calls:
        assert c["home_exists"] is True
        assert c["home_listing"] == []
        assert c["tmpdir"] == c["home"]
    for h in homes:
        assert not Path(h).exists()


def test_lock_runs_when_body_raises(fake: Fake) -> None:
    with pytest.raises(RuntimeError):
        with unlocked(str(fake.bin), PASSWORD, fake.env):
            raise RuntimeError("boom")
    assert fake.argvs()[0][0] == "unlock"
    assert fake.argvs()[-1] == ["lock"]


def test_wrong_password(fake: Fake) -> None:
    with pytest.raises(BwError) as e:
        with unlocked(str(fake.bin), "nope", fake.env):
            pytest.fail("body must not run")
    assert str(e.value) == "wrong master password"


def test_unlock_with_empty_output_is_wrong_password(fake: Fake) -> None:
    with pytest.raises(BwError) as e:
        with unlocked(str(fake.bin), PASSWORD, {**fake.env, "FAKE_UNLOCK": "empty"}):
            pytest.fail("body must not run")
    assert str(e.value) == "wrong master password"


def test_sync_failure_reports_last_sync(fake: Fake) -> None:
    note = sync(str(fake.bin), SESSION, {**fake.env, "FAKE_SYNC": "fail"})
    assert note == "sync failed, data from 2024-01-01T00:00:00.000Z"


def test_sync_failure_never_synced(fake: Fake) -> None:
    st = json.dumps({"serverUrl": None, "lastSync": None, "status": "unlocked"})
    note = sync(str(fake.bin), SESSION, {**fake.env, "FAKE_SYNC": "fail", "FAKE_STATUS": st})
    assert note == "sync failed, data from never"


def test_sync_timeout(fake: Fake) -> None:
    start = time.monotonic()
    note = sync(str(fake.bin), SESSION, {**fake.env, "FAKE_SYNC": "hang"}, timeout=0.5)
    assert time.monotonic() - start < 10
    assert note == "sync failed, data from 2024-01-01T00:00:00.000Z"


def test_sync_calls_status_itself(fake: Fake) -> None:
    sync(str(fake.bin), SESSION, {**fake.env, "FAKE_SYNC": "fail"})
    assert fake.argvs() == [["sync"], ["status"]]


def test_load_errors_per_item(fake: Fake) -> None:
    fake.set_items(
        {
            "ok": [login("ok", "p4ss-ok")],
            "amb": [login("amb", "x"), login("amb", "y")],
            "empty": [login("empty", "")],
            "similar": [login("similar-old", "x")],
        }
    )
    values, errors = load(str(fake.bin), SESSION, ["ok", "amb", "empty", "similar", "none"], fake.env)
    assert values == {"ok": "p4ss-ok"}
    assert errors == {
        "amb": "item ambiguous: amb",
        "empty": "item has no value: empty",
        "similar": "item missing: similar",
        "none": "item missing: none",
    }


def test_load_item_name_passed_verbatim_as_one_arg(fake: Fake) -> None:
    name = "--weird name; $(x)"
    fake.set_items({name: [login(name, "v4lue-x")]})
    values, _ = load(str(fake.bin), SESSION, [name], fake.env)
    assert values == {name: "v4lue-x"}
    assert fake.argvs() == [["list", "items", "--search", name]]
