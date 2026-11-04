"""Publish `secret.request` over NATS via event-pub: a request waits on the console; carries nothing approvable."""

import os
import subprocess

from . import data
from .runner import FIXED_PATH


def notify(title: str) -> None:
    """Run `event-pub secret.request` ($SECRET_BROKER_EVENT_PUB, no-op if unset), 2 s timeout, every exception swallowed."""
    event_pub = os.environ.get("SECRET_BROKER_EVENT_PUB")
    if not event_pub:
        return
    try:
        body = data.dumps({"project": "secret-broker", "title": title, "timeout": 300})
        subprocess.run(
            [event_pub, "secret.request"],
            input=body.encode(),
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            cwd="/",
            # event-pub is a writeShellApplication with nounset, so PATH must be set; natscli wants HOME
            env={"PATH": FIXED_PATH, "HOME": "/var/empty"},
            timeout=2,
            check=False,
        )
    except Exception:
        pass
