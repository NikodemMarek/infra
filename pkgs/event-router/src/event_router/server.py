"""HTTP API: `POST /events` with a JSON event (dispatched to a handler and/or
queued in the inbox), `GET /inbox`, `DELETE /inbox/<id>`, `GET /health`."""

import logging
import re
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from .data import ParseError, dumps, loads
from .event import Event
from .handlers import Dispatcher
from .inbox import Inbox

log = logging.getLogger(__name__)

MAX_BODY = 1 << 20
REAP_INTERVAL = 5.0
INBOX_PATH = "/inbox"
# Browsers can't forge Host or set Content-Type: application/json cross-origin
# without a preflight, so these checks block CSRF and DNS rebinding.
LOCAL_HOST = re.compile(r"(127\.0\.0\.1|localhost|\[::1\])(:[0-9]+)?", re.IGNORECASE)


def _handler(dispatcher: Dispatcher, inbox: Inbox) -> type[BaseHTTPRequestHandler]:
    class Handler(BaseHTTPRequestHandler):
        def _send(self, code: int, body: bytes, content_type: str) -> None:
            self.send_response(code)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def _reply(self, code: int, msg: str) -> None:
            self._send(code, (msg + "\n").encode(), "text/plain; charset=utf-8")

        def _host_ok(self) -> bool:
            host = self.headers.get("Host")
            if host is None or LOCAL_HOST.fullmatch(host):
                return True
            self._reply(403, "forbidden host")
            return False

        def do_GET(self) -> None:
            if not self._host_ok():
                return
            if self.path == "/health":
                self._reply(200, "ok")
            elif self.path == INBOX_PATH:
                body = dumps([i.to_json() for i in inbox.items(time.time())])
                self._send(200, body.encode(), "application/json")
            else:
                self._reply(404, "not found")

        def do_DELETE(self) -> None:
            if not self._host_ok():
                return
            prefix = INBOX_PATH + "/"
            if self.path.startswith(prefix) and inbox.remove(self.path[len(prefix) :]):
                # 204 carries no body, Content-Length or Content-Type
                self.send_response(204)
                self.end_headers()
            else:
                self._reply(404, "not found")

        def do_POST(self) -> None:
            if not self._host_ok():
                return
            if self.path != "/events":
                self._reply(404, "not found")
                return
            content_type = self.headers.get("Content-Type", "")
            media_type = content_type.split(";")[0].strip().lower()
            if media_type != "application/json":
                self._reply(415, "Content-Type must be application/json")
                return
            try:
                length = int(self.headers.get("Content-Length") or "0")
            except ValueError:
                length = -1
            if not 0 < length <= MAX_BODY:
                self._reply(400, f"Content-Length must be 1..{MAX_BODY}")
                return
            try:
                event = Event.from_json(loads(self.rfile.read(length)))
            except ParseError as e:
                self._reply(400, str(e))
                return
            queued = inbox.add(event, time.time()) is not None
            dispatched = dispatcher.dispatch(event)
            if queued or dispatched:
                self._reply(202, "accepted")
            else:
                self._reply(500, f"no runnable handler for {event.describe()}")

        def log_message(self, format: str, *args: object) -> None:
            # access log is noise; dispatches are logged by the Dispatcher
            pass

    return Handler


def _reaper(dispatcher: Dispatcher) -> None:
    while True:
        time.sleep(REAP_INTERVAL)
        dispatcher.reap()


def make_server(port: int, dispatcher: Dispatcher, inbox: Inbox) -> ThreadingHTTPServer:
    return ThreadingHTTPServer(("127.0.0.1", port), _handler(dispatcher, inbox))


def serve(port: int, dispatcher: Dispatcher, inbox: Inbox) -> None:
    server = make_server(port, dispatcher, inbox)
    threading.Thread(target=_reaper, args=(dispatcher,), daemon=True).start()
    log.info("listening on 127.0.0.1:%d", port)
    try:
        server.serve_forever()
    finally:
        server.server_close()
