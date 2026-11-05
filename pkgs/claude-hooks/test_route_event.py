import io
import json
import os
import tempfile
import unittest
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
        with mock.patch.dict(os.environ, {"SHPOOL_SESSION_NAME": "s1", "AGENT_SESSION": "s2"}):
            event = enrich({}, PERM)
        self.assertEqual(event["shpool"], "s1")
        self.assertEqual(event["session"], "s2")

    def test_env_absent_when_empty(self):
        with mock.patch.dict(os.environ, {"SHPOOL_SESSION_NAME": "", "AGENT_SESSION": ""}):
            event = enrich({}, PERM)
        self.assertNotIn("shpool", event)
        self.assertNotIn("session", event)

    def test_session_absent_when_unset(self):
        with mock.patch.dict(os.environ, {}):
            os.environ.pop("AGENT_SESSION", None)
            self.assertNotIn("session", enrich({}, PERM))

    def test_origin_file_never_set(self):
        with mock.patch.dict(os.environ, {"CLAUDE_ORIGIN_FILE": "/o", "AGENT_SESSION": "s"}):
            self.assertNotIn("origin_file", enrich({}, PERM))


class Main(unittest.TestCase):
    def fake_pub(self, code=0, stderr=""):
        """Temp dir with a fake event-pub recording argv and stdin; returns the dir."""
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        path = os.path.join(tmp.name, "event-pub")
        with open(path, "w") as f:
            f.write(f"#!/bin/sh\nprintf '%s\\n' \"$@\" > '{tmp.name}/argv'\ncat > '{tmp.name}/stdin'\n"
                    f"echo '{stderr}' >&2\nexit {code}\n")
        os.chmod(path, 0o755)
        return tmp.name

    def run_main(self, stdin, path_dir=None, argv=("claude-route-event",)):
        """Returns (exit code, stderr); stdout must stay empty. PATH holds only path_dir
        (plus the system dirs of sh/cat for the fake)."""
        out, err = io.StringIO(), io.StringIO()
        path = os.pathsep.join([path_dir, os.environ["PATH"]]) if path_dir else "/nonexistent-bin"
        with (mock.patch("sys.argv", list(argv)),
              mock.patch("sys.stdin", io.StringIO(stdin)),
              mock.patch("sys.stdout", out),
              mock.patch("sys.stderr", err),
              mock.patch.dict(os.environ, {"PATH": path, "SHPOOL_SESSION_NAME": "",
                                           "AGENT_SESSION": ""})):
            code = main()
        self.assertEqual(out.getvalue(), "")
        return code, err.getvalue()

    def test_invalid_input(self):
        for stdin in ("[]", ""):
            self.assertEqual(self.run_main(stdin, self.fake_pub())[0], 1)

    def test_missing_event_pub(self):
        code, err = self.run_main("{}")
        self.assertEqual(code, 1)
        self.assertIn(f"claude-route-event: cannot publish {PERM}", err)

    def test_failing_event_pub(self):
        d = self.fake_pub(1, "boom")
        code, err = self.run_main("{}", d)
        self.assertEqual(code, 1)
        self.assertIn(f"claude-route-event: cannot publish {PERM}: ", err)
        self.assertIn("boom", err)
        self.assertEqual(len(err.strip().splitlines()), 1, err)

    def test_publishes_enriched_event(self):
        d = self.fake_pub()
        code, _ = self.run_main('{"cwd": "/nonexistent", "tool_name": "Read"}', d)
        self.assertEqual(code, 0)
        with open(os.path.join(d, "argv")) as f:
            self.assertEqual(f.read(), PERM + "\n")
        with open(os.path.join(d, "stdin")) as f:
            self.assertEqual(json.load(f), {"cwd": "/nonexistent", "tool_name": "Read", "type": PERM,
                                            "project": "nonexistent", "title": "Read"})

    def test_explicit_type_is_the_argument(self):
        d = self.fake_pub()
        code, _ = self.run_main('{"title": "T"}', d, ("claude-route-event", "sudo.password_request"))
        self.assertEqual(code, 0)
        with open(os.path.join(d, "argv")) as f:
            self.assertEqual(f.read(), "sudo.password_request\n")
        with open(os.path.join(d, "stdin")) as f:
            self.assertEqual(json.load(f)["type"], "sudo.password_request")


if __name__ == "__main__":
    unittest.main()
