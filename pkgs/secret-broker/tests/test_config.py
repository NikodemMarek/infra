import re
from pathlib import Path

import pytest

from secret_broker.config import RESERVED_ENV, ConfigError, load_config, load_config_file
from secret_broker.data import Json, Obj

STORE = "/nix/store/abc-tool/bin/tool"


def act(**over: Json) -> Obj:
    base: Obj = {"description": "d", "command": [STORE]}
    return base | over


def cfg(actions: dict[str, Json] | None = None, **over: Json) -> Obj:
    base: Obj = {"user": "u", "actions": {} if actions is None else actions}
    return base | over


def test_minimal_defaults() -> None:
    c = load_config({"user": "u", "actions": {}})
    assert c.user == "u"
    assert c.lock_after == 240
    assert c.export_provider is None
    assert c.export_profiles == frozenset()
    assert c.bw_server is None
    assert c.actions == {}


def test_full_config() -> None:
    c = load_config(
        {
            "user": "nikodem",
            "lockAfter": 30,
            "exportProvider": "bw://?folder=infra/{profile}",
            "exportProfiles": ["a", "b", "a"],
            "bwServer": "https://vault.example",
            "actions": {
                "deploy-1": {
                    "description": "Deploy",
                    "command": [STORE, "--flag"],
                    "args": ["^[a-z]+$", "^\\d+$"],
                    "secrets": {"TOK": "item-b", "OTHER": "item-a", "DUP": "item-b"},
                    "timeout": 60,
                }
            },
        }
    )
    assert c.lock_after == 30
    assert c.export_provider == "bw://?folder=infra/{profile}"
    assert c.export_profiles == frozenset({"a", "b"})
    assert c.bw_server == "https://vault.example"
    a = c.actions["deploy-1"]
    assert a.name == "deploy-1"
    assert a.description == "Deploy"
    assert a.command == (STORE, "--flag")
    assert a.args == (re.compile("^[a-z]+$"), re.compile("^\\d+$"))
    assert all(isinstance(p, re.Pattern) for p in a.args)
    assert a.args[0].fullmatch("abc")
    assert a.secrets == {"TOK": "item-b", "OTHER": "item-a", "DUP": "item-b"}
    assert a.timeout == 60
    assert a.items() == ("item-a", "item-b")


def test_action_defaults() -> None:
    a = load_config(cfg({"x": act()})).actions["x"]
    assert a.args == ()
    assert a.secrets == {}
    assert a.timeout == 120
    assert a.command == (STORE,)


def test_null_optionals() -> None:
    c = load_config(cfg(exportProvider=None, bwServer=None))
    assert c.export_provider is None
    assert c.bw_server is None


def test_reserved_env_contents() -> None:
    assert RESERVED_ENV == {"PATH", "HOME", "TMPDIR", "LANG", "SSL_CERT_FILE", "NIX_SSL_CERT_FILE"}


@pytest.mark.parametrize("name", ["a", "0", "a-b", "9lives", "a1-2-c"])
def test_valid_names(name: str) -> None:
    assert name in load_config(cfg({name: act()})).actions


@pytest.mark.parametrize(
    "name", ["", "list", "export", "-a", "A", "a_b", "a b", "a.b", "é", "a\n"]
)
def test_bad_names(name: str) -> None:
    with pytest.raises(ConfigError):
        load_config(cfg({name: act()}))


def test_error_names_the_action() -> None:
    with pytest.raises(ConfigError, match="myact"):
        load_config(cfg({"myact": act(command=[])}))


@pytest.mark.parametrize(
    "command",
    [[], ["/bin/sh"], ["sh"], ["/nix/storeX/y"], ["nix/store/x"], ["/usr/bin/env", STORE], [""], ["/nix/store/../../bin/sh"]],
)
def test_bad_command(command: list[str]) -> None:
    with pytest.raises(ConfigError, match="x"):
        load_config(cfg({"x": act(command=command)}))


def test_bad_regex() -> None:
    with pytest.raises(ConfigError, match="x"):
        load_config(cfg({"x": act(args=["("])}))


@pytest.mark.parametrize("env", ["a", "1A", "A-B", "A B", "", "a_B", *sorted(RESERVED_ENV)])
def test_bad_env(env: str) -> None:
    with pytest.raises(ConfigError, match="x"):
        load_config(cfg({"x": act(secrets={env: "item"})}))


@pytest.mark.parametrize("env", ["A", "_", "_A1", "TOKEN_2", "PATHX"])
def test_good_env(env: str) -> None:
    assert load_config(cfg({"x": act(secrets={env: "item"})})).actions["x"].secrets == {env: "item"}


def test_empty_item_name() -> None:
    with pytest.raises(ConfigError, match="x"):
        load_config(cfg({"x": act(secrets={"TOK": ""})}))


@pytest.mark.parametrize("timeout", [0, -1, 3601, 10**6])
def test_bad_timeout(timeout: int) -> None:
    with pytest.raises(ConfigError, match="x"):
        load_config(cfg({"x": act(timeout=timeout)}))


@pytest.mark.parametrize("timeout", [1, 3600])
def test_timeout_bounds_ok(timeout: int) -> None:
    assert load_config(cfg({"x": act(timeout=timeout)})).actions["x"].timeout == timeout


@pytest.mark.parametrize("lock_after", [0, -5, 1441])
def test_bad_lock_after(lock_after: int) -> None:
    with pytest.raises(ConfigError):
        load_config(cfg(lockAfter=lock_after))


@pytest.mark.parametrize("lock_after", [1, 1440])
def test_lock_after_bounds_ok(lock_after: int) -> None:
    assert load_config(cfg(lockAfter=lock_after)).lock_after == lock_after


@pytest.mark.parametrize(
    "raw",
    [
        {"actions": {}},
        {"user": "", "actions": {}},
        {"user": "u"},
        {"user": 1, "actions": {}},
        {"user": "u", "actions": []},
        {"user": "u", "actions": {}, "lockAfter": "5"},
        {"user": "u", "actions": {}, "lockAfter": True},
        {"user": "u", "actions": {}, "exportProvider": 1},
        {"user": "u", "actions": {}, "exportProfiles": "a"},
        {"user": "u", "actions": {}, "exportProfiles": [1]},
        {"user": "u", "actions": {}, "bwServer": 1},
        {"user": "u", "actions": {"x": []}},
        {"user": "u", "actions": {"x": {"command": [STORE]}}},
        {"user": "u", "actions": {"x": {"description": "d"}}},
        {"user": "u", "actions": {"x": {"description": 1, "command": [STORE]}}},
        {"user": "u", "actions": {"x": {"description": "d", "command": STORE}}},
        {"user": "u", "actions": {"x": {"description": "d", "command": [1]}}},
        {"user": "u", "actions": {"x": {"description": "d", "command": [STORE], "args": "x"}}},
        {"user": "u", "actions": {"x": {"description": "d", "command": [STORE], "args": [1]}}},
        {"user": "u", "actions": {"x": {"description": "d", "command": [STORE], "secrets": []}}},
        {"user": "u", "actions": {"x": {"description": "d", "command": [STORE], "secrets": {"A": 1}}}},
        {"user": "u", "actions": {"x": {"description": "d", "command": [STORE], "timeout": "5"}}},
        {"user": "u", "actions": {"x": {"description": "d", "command": [STORE], "timeout": True}}},
    ],
)
def test_wrong_types_and_missing(raw: Obj) -> None:
    with pytest.raises(ConfigError):
        load_config(raw)


def test_file_ok(tmp_path: Path) -> None:
    p = tmp_path / "c.json"
    p.write_text('{"user":"u","actions":{}}')
    assert load_config_file(p).user == "u"


def test_file_invalid_json(tmp_path: Path) -> None:
    p = tmp_path / "c.json"
    p.write_text("{nope")
    with pytest.raises(ConfigError):
        load_config_file(p)


def test_file_not_object(tmp_path: Path) -> None:
    p = tmp_path / "c.json"
    p.write_text("[1]")
    with pytest.raises(ConfigError):
        load_config_file(p)


def test_file_missing(tmp_path: Path) -> None:
    with pytest.raises(ConfigError):
        load_config_file(tmp_path / "absent.json")


def test_file_invalid_config(tmp_path: Path) -> None:
    p = tmp_path / "c.json"
    p.write_text('{"actions":{}}')
    with pytest.raises(ConfigError):
        load_config_file(p)
