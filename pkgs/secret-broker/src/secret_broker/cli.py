"""Run named actions with Bitwarden secrets after approval on a private console."""

import sys
from pathlib import Path

from .config import ConfigError, load_config_file


def main(argv: list[str] | None = None) -> int:
    """`secret-broker check <config.json>`: exit 0 if valid, else `secret-broker: <msg>` on
    stderr and exit 1; usage error exits 64."""
    args = sys.argv[1:] if argv is None else argv
    if len(args) != 2 or args[0] != "check":
        print("usage: secret-broker check <config.json>", file=sys.stderr)
        return 64
    try:
        load_config_file(Path(args[1]))
    except ConfigError as e:
        print(f"secret-broker: {e}", file=sys.stderr)
        return 1
    return 0
