import re
from pathlib import Path

from . import data
from .data import Json, Obj, ParseError
from .model import Action, Config


class ConfigError(Exception):
    pass


RESERVED_ENV: frozenset[str] = frozenset(
    {"PATH", "HOME", "TMPDIR", "LANG", "SSL_CERT_FILE", "NIX_SSL_CERT_FILE"}
)

_ACTION_NAME = re.compile(r"[a-z0-9][a-z0-9-]*")
_ENV_NAME = re.compile(r"[A-Z_][A-Z0-9_]*")
_RESERVED_ACTIONS = frozenset({"list", "export"})


def _action(name: str, raw: Json) -> Action:
    if not _ACTION_NAME.fullmatch(name) or name in _RESERVED_ACTIONS:
        raise ConfigError(f"action {name!r}: invalid name")
    o = data.as_obj(raw, f"actions.{name}")
    command = data.str_list(o.get("command"), "command")
    if not command or not command[0].startswith("/nix/store/"):
        raise ConfigError(f"action {name}: command must start with a /nix/store/ path")
    if ".." in command[0].split("/"):
        raise ConfigError(f"action {name}: command must not contain a .. segment")
    patterns: list[re.Pattern[str]] = []
    for arg in data.str_list(o.get("args", []), "args"):
        try:
            patterns.append(re.compile(arg))
        except re.error as e:
            raise ConfigError(f"action {name}: invalid regex {arg!r}: {e}") from e
    secrets = data.str_dict(o.get("secrets", {}), "secrets")
    for env, item in secrets.items():
        if not _ENV_NAME.fullmatch(env) or env in RESERVED_ENV:
            raise ConfigError(f"action {name}: invalid environment variable {env!r}")
        if not item:
            raise ConfigError(f"action {name}: empty item name for {env}")
    timeout = data.opt_int(o, "timeout", 120)
    if not 1 <= timeout <= 3600:
        raise ConfigError(f"action {name}: timeout must be 1..3600")
    return Action(
        name=name,
        description=data.get_str(o, "description"),
        command=tuple(command),
        args=tuple(patterns),
        secrets=secrets,
        timeout=timeout,
    )


def _config(raw: Obj) -> Config:
    user = data.get_str(raw, "user")
    if not user:
        raise ConfigError("user: must not be empty")
    lock_after = data.opt_int(raw, "lockAfter", 240)
    if not 1 <= lock_after <= 1440:
        raise ConfigError("lockAfter: must be 1..1440")
    actions: dict[str, Action] = {}
    for name, a in data.as_obj(raw.get("actions"), "actions").items():
        try:
            actions[name] = _action(name, a)
        except ParseError as e:
            raise ConfigError(f"action {name}: {e}") from e
    return Config(
        user=user,
        lock_after=lock_after,
        export_provider=data.opt_str(raw, "exportProvider", None),
        export_profiles=frozenset(data.str_list(raw.get("exportProfiles", []), "exportProfiles")),
        bw_server=data.opt_str(raw, "bwServer", None),
        actions=actions,
    )


def load_config(raw: Obj) -> Config:
    try:
        return _config(raw)
    except ParseError as e:
        raise ConfigError(str(e)) from e


def load_config_file(path: Path) -> Config:
    """Read + parse; any ParseError/OSError becomes a ConfigError."""
    try:
        return load_config(data.as_obj(data.loads(path.read_bytes()), "config"))
    except (ParseError, OSError) as e:
        raise ConfigError(f"{path}: {e}") from e
