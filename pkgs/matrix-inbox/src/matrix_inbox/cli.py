"""`matrix-inbox`: the service entry point."""

import logging
import os
import sys
import time
from pathlib import Path
from typing import NoReturn

from .bot import Bot, Fatal, LocalError, MatrixError
from .io import HttpMatrix, ProcessLocal

RETRY_SECONDS = 10

log = logging.getLogger("matrix_inbox")


def _fail(message: str, code: int) -> NoReturn:
    print(f"matrix-inbox: {message}", file=sys.stderr)
    sys.exit(code)


def main() -> None:
    logging.basicConfig(stream=sys.stderr, level=logging.INFO, format="%(levelname)s %(message)s")
    if len(sys.argv) > 1:
        print("usage: matrix-inbox", file=sys.stderr)
        sys.exit(2)
    homeserver = os.environ.get("MATRIX_HOMESERVER")
    owner = os.environ.get("MATRIX_OWNER")
    token_file = os.environ.get("MATRIX_TOKEN_FILE")
    if not homeserver or not owner or not token_file:
        _fail("set MATRIX_HOMESERVER, MATRIX_OWNER and MATRIX_TOKEN_FILE", 2)
    try:
        token = Path(token_file).read_text().strip()
    except OSError as e:
        _fail(f"cannot read the token file: {e}", 2)
    bot = Bot(HttpMatrix(homeserver, token), ProcessLocal(), owner)
    try:
        while True:
            try:
                bot.start()
                break
            except MatrixError as e:
                log.warning("start failed: %s", e)
                time.sleep(RETRY_SECONDS)
        while True:
            try:
                bot.step()
            except (MatrixError, LocalError) as e:
                log.warning("step failed: %s", e)
                time.sleep(RETRY_SECONDS)
    except Fatal as e:
        log.error("%s", e)
        sys.exit(1)
