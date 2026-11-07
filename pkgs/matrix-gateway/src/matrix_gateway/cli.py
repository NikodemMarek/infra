"""`matrix-gateway`: the service entry point; `matrix-gateway keygen` makes a signing key pair."""

import base64
import logging
import os
import sys
import threading
import time
import uuid
from pathlib import Path
from typing import NoReturn

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

from .gateway import Fatal, Gateway, MatrixError
from .io import HttpMatrix, NatsBus, NatsSub

RETRY_SECONDS = 10
SEND_ATTEMPTS = 3
SEND_RETRY_SECONDS = 5
DEFAULT_NATS_URL = "nats://127.0.0.1:4222"

log = logging.getLogger("matrix_gateway")


def _fail(message: str, code: int) -> NoReturn:
    print(f"matrix-gateway: {message}", file=sys.stderr)
    sys.exit(code)


def _deliver(gateway: Gateway, line: str) -> None:
    txn = "send-" + uuid.uuid4().hex
    for attempt in range(1, SEND_ATTEMPTS + 1):
        try:
            gateway.on_send(line, txn)
            return
        except MatrixError as e:
            log.warning("posting %s failed (attempt %d of %d): %s", txn, attempt, SEND_ATTEMPTS, e)
            if attempt < SEND_ATTEMPTS:
                time.sleep(SEND_RETRY_SECONDS)
    log.error("dropped %s", txn)


def _pump(gateway: Gateway, sub: NatsSub) -> NoReturn:
    try:
        for line in sub:
            _deliver(gateway, line)
        log.error("nats sub ended")
    except Fatal as e:
        log.error("%s", e)
    except OSError as e:
        log.error("nats sub failed: %s", e)
    except BaseException:
        log.exception("nats pump crashed")
    os._exit(1)


def main() -> None:
    logging.basicConfig(stream=sys.stderr, level=logging.INFO, format="%(levelname)s %(message)s")
    if sys.argv[1:] == ["keygen"]:
        keygen()
        return
    if len(sys.argv) > 1:
        print("usage: matrix-gateway [keygen]", file=sys.stderr)
        sys.exit(2)
    homeserver = os.environ.get("MATRIX_HOMESERVER")
    owner = os.environ.get("MATRIX_OWNER")
    user = os.environ.get("MATRIX_USER")
    password_file = os.environ.get("MATRIX_PASSWORD_FILE")
    key_file = os.environ.get("MATRIX_SIGNING_KEY_FILE")
    if not homeserver or not owner or not user or not password_file or not key_file:
        _fail("set MATRIX_HOMESERVER, MATRIX_OWNER, MATRIX_USER, MATRIX_PASSWORD_FILE and MATRIX_SIGNING_KEY_FILE", 2)
    nats_url = os.environ.get("NATS_URL") or DEFAULT_NATS_URL
    try:
        password = Path(password_file).read_text().strip()
        key_text = Path(key_file).read_text().strip()
    except OSError as e:
        _fail(f"cannot read a secret file: {e}", 2)
    try:
        key = Ed25519PrivateKey.from_private_bytes(base64.b64decode(key_text, validate=True))
    except ValueError as e:
        _fail(f"invalid signing key: {e}", 2)
    gateway = Gateway(HttpMatrix(homeserver), NatsBus(nats_url), owner, user, password, key)
    try:
        while True:
            try:
                gateway.start()
                break
            except MatrixError as e:
                log.warning("start failed: %s", e)
                time.sleep(RETRY_SECONDS)
        threading.Thread(target=_pump, args=(gateway, NatsSub(nats_url)), daemon=True).start()
        while True:
            try:
                gateway.step()
            except MatrixError as e:
                log.warning("step failed: %s", e)
                time.sleep(RETRY_SECONDS)
    except Fatal as e:
        log.error("%s", e)
        sys.exit(1)


def keygen() -> None:
    key = Ed25519PrivateKey.generate()
    private = base64.b64encode(key.private_bytes_raw()).decode()
    public = base64.b64encode(key.public_key().public_bytes_raw()).decode()
    print(f"GATEWAY_SIGNING_KEY={private}")
    print(f"MATRIX_GATEWAY_KEY={public}")
