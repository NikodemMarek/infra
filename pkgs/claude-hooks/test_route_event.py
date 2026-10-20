import io
import json
import os
import threading
import unittest
from http.server import BaseHTTPRequestHandler, HTTPServer
from unittest import mock

from route_event import enrich, event_type, main, title

PERM = "claude.permission_request"


class EventType(unittest.TestCase):
    def test_argument_wins(self):
        self.assertEqual(event_type("sudo.password_request", {"tool_name": "AskUserQuestion"}),
                         "sudo.password_request")

    def test_question(self):
        self.assertEqual(event_type("", {"tool_name": "AskUserQuestion"}), "claude.question")

    def test_default(self):
        self.assertEqual(event_type("", {"tool_name": "Bash"}), PERM)
        self.assertEqual(event_type("", {}), PERM)


class Title(unittest.TestCase):
    def test_explicit_title(self):
        inp = {"title": "T", "tool_name": "Bash", "tool_input": {"description": "D"}}
        self.assertEqual(title(inp, PERM), "T")

    def test_empty_or_non_string_title_is_skipped(self):
        self.assertEqual(title({"title": "", "tool_name": "Read"}, PERM), "Read")
        self.assertEqual(title({"title": 5, "tool_name": "Read"}, PERM), "Read")

    def test_question(self):
        inp = {"tool_name": "AskUserQuestion", "tool_input": {"questions": [{"question": "Which?"}]}}
        self.assertEqual(title(inp, "claude.question"), "Which?")

    def test_malformed_questions(self):
        for questions in ([], [None], [{"question": ""}], [{"question": 1}], "x", None):
            inp = {"tool_name": "AskUserQuestion", "tool_input": {"questions": questions}}
            self.assertEqual(title(inp, "claude.question"), "AskUserQuestion")

    def test_bash_description(self):
        inp = {"tool_name": "Bash", "tool_input": {"description": "List", "command": "ls"}}
        self.assertEqual(title(inp, PERM), "List")

    def test_bash_command_first_line(self):
        inp = {"tool_name": "Bash", "tool_input": {"description": "", "command": "echo a\necho b"}}
        self.assertEqual(title(inp, PERM), "echo a")

    def test_bash_command_leading_blank_lines(self):
        inp = {"tool_name": "Bash", "tool_input": {"command": "\n\necho a\necho b"}}
        self.assertEqual(title(inp, PERM), "echo a")

    def test_bash_without_description_and_command(self):
        inp = {"tool_name": "Bash", "tool_input": {"command": ""}}
        self.assertEqual(title(inp, PERM), "Bash")

    def test_description_only_counts_for_bash(self):
        inp = {"tool_name": "Task", "tool_input": {"description": "D"}}
        self.assertEqual(title(inp, PERM), "Task")

    def test_file_path_relative_to_cwd(self):
        inp = {"tool_name": "Edit", "cwd": "/a/b", "tool_input": {"file_path": "/a/b/src/x.py"}}
        self.assertEqual(title(inp, PERM), "Edit src/x.py")

    def test_file_path_outside_cwd(self):
        inp = {"tool_name": "Edit", "cwd": "/a/b", "tool_input": {"file_path": "/a/bc/x.py"}}
        self.assertEqual(title(inp, PERM), "Edit /a/bc/x.py")

    def test_file_path_without_cwd(self):
        inp = {"tool_name": "Read", "tool_input": {"file_path": "/a/x.py"}}
        self.assertEqual(title(inp, PERM), "Read /a/x.py")
        inp["cwd"] = ""
        self.assertEqual(title(inp, PERM), "Read /a/x.py")

    def test_url(self):
        inp = {"tool_name": "WebFetch", "tool_input": {"url": "https://x.y"}}
        self.assertEqual(title(inp, PERM), "WebFetch https://x.y")

    def test_empty_url_and_file_path(self):
        inp = {"tool_name": "WebFetch", "tool_input": {"url": "", "file_path": ""}}
        self.assertEqual(title(inp, PERM), "WebFetch")

    def test_tool_name(self):
        self.assertEqual(title({"tool_name": "Glob", "tool_input": {}}, PERM), "Glob")

    def test_type(self):
        self.assertEqual(title({}, "claude.waiting"), "claude.waiting")
        self.assertEqual(title({"tool_name": ""}, "claude.waiting"), "claude.waiting")

    def test_tool_input_not_a_dict(self):
        for tool_input in (None, "ls", ["a"], 5):
            inp = {"tool_name": "Bash", "tool_input": tool_input}
            self.assertEqual(title(inp, PERM), "Bash")

    def test_non_string_values(self):
        inp = {"tool_name": "Bash", "cwd": 3, "tool_input": {"description": 1, "command": ["ls"],
                                                             "file_path": 2, "url": {}}}
        self.assertEqual(title(inp, PERM), "Bash")
        self.assertEqual(title({"tool_name": 7}, PERM), PERM)


class Enrich(unittest.TestCase):
    def test_project_title_and_no_jj_outside_a_checkout(self):
        event = enrich({"cwd": "/nonexistent", "tool_name": "Read"}, PERM)
        self.assertEqual(event["project"], "nonexistent")
        self.assertEqual(event["title"], "Read")
        self.assertEqual(event["type"], PERM)
        self.assertNotIn("jj", event)

    def test_no_cwd_gives_null_project(self):
        self.assertIsNone(enrich({}, PERM)["project"])

    def test_env_added_when_set(self):
        with mock.patch.dict(os.environ, {"SHPOOL_SESSION_NAME": "s1", "CLAUDE_ORIGIN_FILE": "/o"}):
            event = enrich({}, PERM)
        self.assertEqual(event["shpool"], "s1")
        self.assertEqual(event["origin_file"], "/o")

    def test_env_absent_when_empty(self):
        with mock.patch.dict(os.environ, {"SHPOOL_SESSION_NAME": "", "CLAUDE_ORIGIN_FILE": ""}):
            event = enrich({}, PERM)
        self.assertNotIn("shpool", event)
        self.assertNotIn("origin_file", event)


class Main(unittest.TestCase):
    def run_main(self, stdin, url="http://127.0.0.1:1"):
        out = io.StringIO()
        with (mock.patch("sys.argv", ["claude-route-event"]),
              mock.patch("sys.stdin", io.StringIO(stdin)),
              mock.patch("sys.stdout", out),
              mock.patch("sys.stderr", io.StringIO()),
              mock.patch.dict(os.environ, {"EVENT_ROUTER_URL": url, "SHPOOL_SESSION_NAME": "",
                                           "CLAUDE_ORIGIN_FILE": ""})):
            code = main()
        self.assertEqual(out.getvalue(), "")
        return code

    def test_invalid_input(self):
        for stdin in ("[]", ""):
            self.assertEqual(self.run_main(stdin), 1)

    def test_unreachable_router(self):
        self.assertEqual(self.run_main("{}"), 1)

    def test_posts_enriched_event(self):
        bodies = []

        class Handler(BaseHTTPRequestHandler):
            def do_POST(self):
                bodies.append((self.path, json.loads(self.rfile.read(int(self.headers["Content-Length"])))))
                self.send_response(204)
                self.end_headers()

            def log_message(self, *args):
                pass

        server = HTTPServer(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=server.serve_forever)
        thread.start()
        try:
            code = self.run_main('{"cwd": "/nonexistent", "tool_name": "Read"}',
                                 f"http://127.0.0.1:{server.server_port}")
        finally:
            server.shutdown()
            server.server_close()
            thread.join()
        self.assertEqual(code, 0)
        self.assertEqual(bodies, [("/events", {"cwd": "/nonexistent", "tool_name": "Read", "type": PERM,
                                               "project": "nonexistent", "title": "Read"})])


if __name__ == "__main__":
    unittest.main()
