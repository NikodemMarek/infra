import os
import sys
import tempfile
import unittest
from unittest import mock

import jw
from jw import Fail, attach, match_session, session_rows


class MatchSession(unittest.TestCase):
    def test_exact_wins_over_longer(self):
        self.assertEqual(match_session(["ab", "abc"], "ab"), "ab")

    def test_unique_prefix(self):
        self.assertEqual(match_session(["foo-1", "bar-2"], "fo"), "foo-1")

    def test_none(self):
        with self.assertRaises(Fail) as c:
            match_session(["foo"], "x")
        self.assertEqual(str(c.exception), "no Claude shpool session x")

    def test_ambiguous(self):
        with self.assertRaises(Fail) as c:
            match_session(["fb", "fa", "g"], "f")
        self.assertEqual(str(c.exception), "f is ambiguous: fa, fb")


class SessionRows(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = tmp.name
        p = mock.patch.object(jw, "ROOT", self.root)
        p.start()
        self.addCleanup(p.stop)

    def jj(self, *parts):
        d = os.path.join(self.root, *parts)
        os.makedirs(os.path.join(d, ".jj"))
        return d

    def test_rows_order_and_where(self):
        a = self.jj("alpha")
        b = self.jj("beta.agents", "w1")
        sub = os.path.join(b, "src", "x")
        os.makedirs(sub)
        found = {"s-b": sub, "s-a": a, "s-c": a, "s-gone": a}
        state = {"s-b": "attached", "s-a": "disconnected", "s-c": "attached"}
        self.assertEqual(session_rows(found, state, "beta"), [
            "s-b\tbeta/w1\tattached",
            "s-a\talpha/default\tdisconnected",
            "s-c\talpha/default\tattached",
        ])

    def test_no_current(self):
        a = self.jj("alpha")
        b = self.jj("beta")
        rows = session_rows({"y": a, "x": b}, {"y": "attached", "x": "attached"}, None)
        self.assertEqual(rows, ["x\tbeta/default\tattached", "y\talpha/default\tattached"])

    def test_no_jj_dir_uses_cwd(self):
        d = os.path.join(self.root, "plain")
        os.makedirs(d)
        self.assertEqual(session_rows({"s": d}, {"s": "attached"}, "alpha"),
                         [f"s\t{d}\tattached"])


class Attach(unittest.TestCase):
    def test_exit_code(self):
        with mock.patch("subprocess.run") as run:
            run.return_value = mock.Mock(returncode=3)
            with self.assertRaises(SystemExit) as c:
                attach("n")
        self.assertEqual(c.exception.code, 3)
        run.assert_called_once_with([jw.SHPOOL, "--no-daemonize", "attach", "n"])

    def test_oserror(self):
        with mock.patch("subprocess.run", side_effect=OSError("boom")):
            with self.assertRaises(Fail) as c:
                attach("n")
        self.assertTrue(str(c.exception).startswith("shpool: "))


class Cli(unittest.TestCase):
    def parse(self, *args):
        with mock.patch.object(sys, "argv", ["jw", *args]):
            return jw.parse_args()

    def test_attach(self):
        a = self.parse("attach", "foo")
        self.assertIs(a.func, jw.cmd_attach)
        self.assertEqual(a.name, "foo")

    def test_alias(self):
        a = self.parse("at", "foo")
        self.assertIs(a.func, jw.cmd_attach)
        self.assertEqual(a.name, "foo")

    def test_name_optional(self):
        a = self.parse("attach")
        self.assertIs(a.func, jw.cmd_attach)
        self.assertFalse(a.name)


if __name__ == "__main__":
    unittest.main()
