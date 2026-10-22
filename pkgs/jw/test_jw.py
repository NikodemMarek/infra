import contextlib
import io
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

    def test_reap(self):
        a = self.parse("reap")
        self.assertIs(a.func, jw.cmd_reap)

    def test_reap_rejects_args(self):
        with mock.patch("sys.stderr"), self.assertRaises(SystemExit):
            self.parse("reap", "x")


class SessionTop(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.proc = tmp.name

    def add(self, pid, ppid, comm, stat=None):
        d = os.path.join(self.proc, str(pid))
        os.makedirs(d, exist_ok=True)
        with open(os.path.join(d, "stat"), "w") as f:
            f.write(stat if stat is not None else f"{pid} (x) S {ppid} 1 1")
        if comm is not None:
            with open(os.path.join(d, "comm"), "w") as f:
                f.write(comm + "\n")

    def test_direct_shpool(self):
        self.add(10, 1, "x")
        self.add(1, 0, "shpool")
        self.assertTrue(jw.session_top(10, self.proc))

    def test_via_event_claude_la(self):
        self.add(10, 20, "x")
        self.add(20, 1, "event-claude-la")
        self.add(1, 0, "shpool")
        self.assertTrue(jw.session_top(10, self.proc))

    def test_event_claude_la_under_bash(self):
        self.add(10, 20, "x")
        self.add(20, 30, "event-claude-la")
        self.add(30, 1, "bash")
        self.assertFalse(jw.session_top(10, self.proc))

    def test_bash_between(self):
        self.add(10, 20, "x")
        self.add(20, 1, "bash")
        self.add(1, 0, "shpool")
        self.assertFalse(jw.session_top(10, self.proc))

    def test_flock_parent(self):
        self.add(10, 20, "x")
        self.add(20, 1, "flock")
        self.assertFalse(jw.session_top(10, self.proc))

    def test_missing_stat(self):
        self.assertFalse(jw.session_top(10, self.proc))

    def test_malformed_stat(self):
        self.add(10, 1, "x", stat="garbage")
        self.add(1, 0, "shpool")
        self.assertFalse(jw.session_top(10, self.proc))

    def test_missing_comm(self):
        self.add(10, 1, "x")
        self.add(1, 0, None)
        self.assertFalse(jw.session_top(10, self.proc))


class IdleSessions(unittest.TestCase):
    NOW = 1_000_000_000
    H4 = 4 * 3600

    def setUp(self):
        p = mock.patch.object(jw, "session_top", side_effect=lambda pid, proc="/proc": pid not in self.nontop)
        p.start()
        self.addCleanup(p.stop)
        self.nontop = set()

    def claude(self, pid, name, age=None, status="idle", sid="sid", cwd="/w", **kw):
        s = {"status": status, "sessionId": sid}
        if age is not None:
            s["statusUpdatedAt"] = (self.NOW - age) * 1000
        s.update(kw)
        if sid is None:
            del s["sessionId"]
        return (pid, s, cwd, name)

    def run_(self, claudes, state=None):
        if state is None:
            state = {c[3]: "disconnected" for c in claudes}
        return jw.idle_sessions(claudes, state, self.NOW, "/nonexistent")

    def test_limit(self):
        self.assertEqual(jw.IDLE_LIMIT, 4 * 3600)

    def test_exactly_limit(self):
        self.assertEqual(self.run_([self.claude(1, "n", self.H4)]), [("n", "/w", "sid")])

    def test_just_under(self):
        self.assertEqual(self.run_([self.claude(1, "n", self.H4 - 1)]), [])

    def test_busy(self):
        self.assertEqual(self.run_([self.claude(1, "n", self.H4 + 100, status="busy")]), [])

    def test_permission_prompt(self):
        c = self.claude(1, "n", self.H4, status="waiting", waitingFor="permission prompt")
        self.assertEqual(self.run_([c]), [("n", "/w", "sid")])

    def test_other_waits_and_shell(self):
        for c in (self.claude(1, "n", 5 * 3600, status="waiting", waitingFor="input needed"),
                  self.claude(1, "n", 5 * 3600, status="waiting"),
                  self.claude(1, "n", 5 * 3600, status="shell")):
            self.assertEqual(self.run_([c]), [], c)

    def test_bad_updated_at(self):
        c = self.claude(1, "n", sid="sid")
        self.assertEqual(self.run_([c]), [])
        c[1]["statusUpdatedAt"] = None
        self.assertEqual(self.run_([c]), [])
        c[1]["statusUpdatedAt"] = "123"
        self.assertEqual(self.run_([c]), [])

    def test_attached(self):
        self.assertEqual(self.run_([self.claude(1, "n", 5 * 3600)], {"n": "attached"}), [])

    def test_name_absent_from_state(self):
        self.assertEqual(self.run_([self.claude(1, "n", 5 * 3600)], {}), [])

    def test_not_top(self):
        self.nontop = {1}
        self.assertEqual(self.run_([self.claude(1, "n", 5 * 3600)]), [])

    def test_name_none_ignored(self):
        cs = [self.claude(1, None, 5 * 3600), self.claude(2, "n", 5 * 3600)]
        self.assertEqual(self.run_(cs, {"n": "disconnected"}), [("n", "/w", "sid")])

    def test_same_name_mixed(self):
        self.nontop = {2}
        cs = [self.claude(1, "n", 5 * 3600), self.claude(2, "n", 5 * 3600)]
        self.assertEqual(self.run_(cs), [])

    def test_sorted_and_filtered(self):
        cs = [self.claude(1, "b", 5 * 3600, sid="sb", cwd="/b"),
              self.claude(2, "a", 5 * 3600, sid="sa", cwd="/a"),
              self.claude(3, "c", 5 * 3600)]
        state = {"a": "disconnected", "b": "disconnected", "c": "attached"}
        self.assertEqual(self.run_(cs, state), [("a", "/a", "sa"), ("b", "/b", "sb")])

    def test_empty(self):
        self.assertEqual(self.run_([], {}), [])

    def test_missing_session_id(self):
        self.assertEqual(self.run_([self.claude(1, "n", self.H4, sid=None)]), [("n", "/w", "?")])


class CmdReap(unittest.TestCase):
    def reap(self, idle=None, run=None, status=None):
        out, err = io.StringIO(), io.StringIO()
        patches = [
            mock.patch.object(jw, "idle_sessions", return_value=idle or []),
            mock.patch.object(jw, "live_claudes", return_value=["claudes"]),
            mock.patch.object(jw, "shpool_status", **(status or {"return_value": {"s": "x"}})),
            mock.patch.object(jw.time, "time", return_value=42.0),
            mock.patch("subprocess.run", **(run or {"return_value": mock.Mock(returncode=0)})),
            mock.patch("sys.stdout", out),
            mock.patch("sys.stderr", err),
        ]
        with contextlib.ExitStack() as st:
            m = [st.enter_context(p) for p in patches]
            try:
                self.result = jw.cmd_reap(mock.Mock())
                self.exc = None
            except Exception as e:
                self.exc = e
        self.idle_m, self.run_m = m[0], m[4]
        return out.getvalue(), err.getvalue()

    def test_nothing(self):
        out, err = self.reap([])
        self.assertEqual((out, err), ("", ""))
        self.assertIsNone(self.result)
        self.assertIsNone(self.exc)
        self.idle_m.assert_called_once_with(["claudes"], {"s": "x"}, 42.0)
        self.run_m.assert_not_called()

    def test_stops(self):
        out, err = self.reap([("n", "/w", "sid")])
        self.assertIsNone(self.exc)
        self.assertEqual(out, "stopped n in /w: claude --resume sid\n")
        self.assertEqual(self.run_m.call_args_list[0].args[0], [jw.SHPOOL, "--no-daemonize", "kill", "n"])

    def test_failure_continues_then_fails(self):
        rets = [mock.Mock(returncode=1), mock.Mock(returncode=0)]
        out, err = self.reap([("a", "/a", "sa"), ("b", "/b", "sb")], run={"side_effect": rets})
        self.assertIsInstance(self.exc, Fail)
        self.assertEqual(err, "jw: shpool kill a failed\n")
        self.assertEqual(out, "stopped b in /b: claude --resume sb\n")
        self.assertEqual(self.run_m.call_count, 2)

    def test_oserror(self):
        out, err = self.reap([("a", "/a", "sa")], run={"side_effect": OSError("x")})
        self.assertIsInstance(self.exc, Fail)
        self.assertEqual(err, "jw: shpool kill a failed\n")
        self.assertEqual(out, "")

    def test_status_fail_propagates(self):
        self.reap([], status={"side_effect": Fail("nope")})
        self.assertIsInstance(self.exc, Fail)
        self.assertEqual(str(self.exc), "nope")

    def test_no_jj(self):
        self.reap([("a", "/a", "sa")])
        self.run_m.assert_called()
        for c in self.run_m.call_args_list:
            self.assertNotIn("jj", c.args[0])


if __name__ == "__main__":
    unittest.main()
