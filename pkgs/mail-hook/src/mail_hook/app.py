"""`mail-hook`: HTTP ingress that publishes `mail.received` events to NATS."""

import hmac
import json
import os
import subprocess
import sys
from collections.abc import Callable, Mapping
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import NoReturn

MAX_BODY = 65536
PUBLISH_TIMEOUT = 5
KINDS = {"throwaway", "hook"}
FIELDS = ("from", "to", "subject", "snippet")


class PublishError(Exception):
    pass


def _event(body: bytes) -> str | None:
    try:
        data = json.loads(body)
    except ValueError:
        return None
    if not isinstance(data, dict):
        return None
    kind = data.get("kind")
    if kind not in KINDS:
        return None
    event = {"type": "mail.received", "kind": kind}
    for field in FIELDS:
        value = data.get(field)
        if not isinstance(value, str):
            return None
        event[field] = value
    return json.dumps(event)


def handle(
    method: str,
    path: str,
    headers: Mapping[str, str],
    body: bytes,
    token: str,
    publish: Callable[[str, str], None],
) -> int:
    if path != "/mail":
        return 404
    if method != "POST":
        return 405
    authorization = next((v for k, v in headers.items() if k.lower() == "authorization"), "")
    if not hmac.compare_digest(authorization.encode(), f"Bearer {token}".encode()):
        return 401
    if len(body) > MAX_BODY:
        return 413
    line = _event(body)
    if line is None:
        return 400
    try:
        publish("mail.received", line)
    except PublishError:
        return 502
    return 204


def _fail(message: str) -> NoReturn:
    print(f"mail-hook: {message}", file=sys.stderr)
    sys.exit(2)


def main() -> None:
    nats_url = os.environ.get("NATS_URL")
    token_file = os.environ.get("MAIL_HOOK_TOKEN_FILE")
    if not nats_url or not token_file:
        _fail("set NATS_URL and MAIL_HOOK_TOKEN_FILE")
    try:
        token = Path(token_file).read_text().strip()
    except OSError as e:
        _fail(f"cannot read the token file: {e}")
    if not token:
        _fail("the token file is empty")

    def publish(subject: str, line: str) -> None:
        try:
            done = subprocess.run(
                ["nats", "--server", nats_url, "pub", "--no-templates", subject],
                input=line,
                capture_output=True,
                encoding="utf-8",
                errors="replace",
                timeout=PUBLISH_TIMEOUT,
            )
        except (OSError, subprocess.SubprocessError) as e:
            raise PublishError(f"nats pub: {e}") from e
        if done.returncode != 0:
            raise PublishError(done.stderr.strip() or f"exit {done.returncode}")

    class Handler(BaseHTTPRequestHandler):
        def _respond(self) -> None:
            length = self.headers.get("Content-Length", "")
            size = int(length) if length.isascii() and length.isdigit() else 0
            if size > MAX_BODY:
                code = 413
            else:
                body = self.rfile.read(size) if size else b""
                code = handle(self.command, self.path, dict(self.headers.items()), body, token, publish)
            self.send_response(code)
            if code != 204:
                self.send_header("Content-Length", "0")
            self.end_headers()

        do_GET = do_POST = do_PUT = do_PATCH = do_DELETE = _respond

        def log_message(self, format: str, *args: object) -> None:
            pass

    ThreadingHTTPServer(("0.0.0.0", 8080), Handler).serve_forever()
