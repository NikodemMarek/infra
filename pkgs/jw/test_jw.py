import contextlib
import fcntl
import io
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest import mock

import jw
from jw import Fail, attach, match_session, session_rows


class TaskRecord(unittest.TestCase):
    def test_empty_prompt(self):
        self.assertEqual(jw.task_record("", "/w"), {"kind": "task", "workspace": "/w"})

    def test_title_and_description(self):
        prompt = "  \n fix x \nmore"
        r = jw.task_record(prompt, "/w")
        self.assertEqual(r["kind"], "task")
        self.assertEqual(r["workspace"], "/w")
        self.assertEqual(r["title"], "fix x")
        self.assertEqual(r["description"], prompt)

    def test_caps(self):
        r = jw.task_record("t" * 300 + "\n" + "d" * 9000, "/w")
        self.assertEqual(r["title"], "t" * 200)
        self.assertEqual(len(r["description"]), 8000)


BAD = b"fix \xff x".decode("utf-8", "surrogateescape")


class TaskRecordSurrogate(unittest.TestCase):
    def test_valid_utf8_and_writable(self):
        r = jw.task_record(BAD + "\nmore", "/w")
        for k in ("title", "description"):
            r[k].encode("utf-8")  # must not raise
            self.assertIn("\ufffd", r[k])
        self.assertEqual(r["title"], "fix \ufffd x")
        d = tempfile.mkdtemp(dir="/tmp")
        self.addCleanup(shutil.rmtree, d, True)
        out = jw.update_record(d, "s-1", r)
        self.assertEqual(jw.read_record(d, "s-1"), out)
        self.assertEqual(out["title"], "fix \ufffd x")


class AgentEnv(unittest.TestCase):
    def test_drops_session_marker(self):
        env = jw.agent_env({"AGENT_SESSION": "s", "CLAUDECODE": "1", "KEEP": "1"})
        self.assertEqual(env, {"KEEP": "1"})

    def test_drops_calling_claude_session(self):
        env = jw.agent_env({"CLAUDE_CODE_SESSION_ID": "x", "CLAUDE_CODE_CHILD_SESSION": "1", "CLAUDE_PID": "7",
                            "CLAUDE_CODE_MESSAGING_SOCKET": "/s", "CLAUDE_CODE_MESSAGING_TOKEN": "t",
                            "CLAUDE_CODE_PROJECT_DIR_NAME": "p", "CLAUDE_EFFORT": "medium", "AI_AGENT": "a",
                            "CLAUDE_CONFIG_DIR": "/c", "KEEP": "1"})
        self.assertEqual(env, {"CLAUDE_CONFIG_DIR": "/c", "KEEP": "1"})

    def test_origin_file_no_longer_special(self):
        self.assertEqual(jw.agent_env({"CLAUDE_ORIGIN_FILE": "/o"}), {"CLAUDE_ORIGIN_FILE": "/o"})


class CmdAgent(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(dir="/tmp")
        self.addCleanup(shutil.rmtree, self.tmp, True)
        self.state = os.path.join(self.tmp, "state")
        self.run_dir = os.path.join(self.tmp, "run")
        os.makedirs(self.run_dir)
        self.work = os.path.join(self.tmp, "work")
        os.makedirs(self.work)

    def agent(self, update=None, prompt="fix x\nmore"):
        """Runs cmd_agent -d with a fake shpool; returns stderr. Sets self.job (script text), self.at_attach
        (record file content when shpool ran), self.attach_m."""
        self.job = self.at_attach = None
        rec = os.path.join(self.state, "agent-sessions", "s-1.json")

        def run(argv, *a, **kw):
            self.job = open(argv[argv.index("-c") + 1].split()[-1], errors="surrogateescape").read()
            if os.path.exists(rec):
                self.at_attach = open(rec).read()
            return mock.Mock(returncode=0)

        err = io.StringIO()
        env = {"XDG_STATE_HOME": self.state, "XDG_RUNTIME_DIR": self.run_dir, "KNOWLEDGE_ORIGIN": "",
               "AGENT_SESSION": "outer"}
        patches = [
            mock.patch.dict(os.environ, env),
            mock.patch.object(jw, "locate", return_value=("x", self.tmp)),
            mock.patch.object(jw, "fetch"),
            mock.patch.object(jw, "add_workspace", return_value=self.work),
            mock.patch.object(jw, "shpool_status", side_effect=[{}, {"s-1": "detached"}]),
            mock.patch.object(jw.time, "strftime", return_value="s-1"),
            mock.patch.object(jw, "attach"),
            mock.patch("subprocess.run", side_effect=run),
            mock.patch("sys.stderr", err),
        ]
        if update:
            patches.append(mock.patch.object(jw, "update_record", side_effect=update))
        with contextlib.ExitStack() as st:
            ms = [st.enter_context(p) for p in patches]
            self.attach_m = ms[6]
            jw.cmd_agent(mock.Mock(args=["--", prompt], revision=None, detach=False))
        return err.getvalue()

    def test_session_env_and_record(self):
        self.agent()
        self.assertIn("export AGENT_SESSION=s-1\n", self.job)
        self.assertNotIn("outer", self.job)
        self.assertIsNotNone(self.at_attach, "record must exist before shpool attach")
        r = json.loads(self.at_attach)
        self.assertEqual(r["kind"], "task")
        self.assertEqual(r["title"], "fix x")
        self.assertEqual(r["workspace"], self.work)
        self.attach_m.assert_called_once_with("s-1")

    def test_record_failure_warns_and_starts(self):
        for exc in (Fail("boom"), OSError("disk")):
            with self.subTest(exc=exc):
                err = self.agent(update=exc)
                self.assertIn("jw: cannot write the session record:", err)
                self.assertIsNotNone(self.job)
                self.attach_m.assert_called_once_with("s-1")

    def test_surrogate_prompt_still_starts(self):
        err = self.agent(prompt=BAD + "\nmore")
        self.assertNotIn("cannot write the session record", err)
        self.assertIsNotNone(self.job)
        self.attach_m.assert_called_once_with("s-1")
        r = json.loads(self.at_attach)
        self.assertEqual(r["title"], "fix \ufffd x")


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
        self.assertEqual(out, "stopped n in /w: AGENT_SESSION=n claude --resume sid\n")
        self.assertEqual(self.run_m.call_args_list[0].args[0], [jw.SHPOOL, "--no-daemonize", "kill", "n"])

    def test_failure_continues_then_fails(self):
        rets = [mock.Mock(returncode=1), mock.Mock(returncode=0)]
        out, err = self.reap([("a", "/a", "sa"), ("b", "/b", "sb")], run={"side_effect": rets})
        self.assertIsInstance(self.exc, Fail)
        self.assertEqual(err, "jw: shpool kill a failed\n")
        self.assertEqual(out, "stopped b in /b: AGENT_SESSION=b claude --resume sb\n")
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


class CmdMsg(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(dir="/tmp")  # short: AF_UNIX paths are limited to ~100 bytes
        self.addCleanup(shutil.rmtree, self.tmp, True)
        self.sessions = os.path.join(self.tmp, "sessions")
        os.makedirs(self.sessions)
        env = mock.patch.dict(os.environ, {"CLAUDE_CONFIG_DIR": self.tmp, "KNOWLEDGE_ORIGIN": "",
                                           "SHPOOL_SESSION_NAME": "me"})
        env.start()
        self.addCleanup(env.stop)
        w = mock.patch.object(jw, "MSG_WAIT", 0)
        w.start()
        self.addCleanup(w.stop)
        self.received = []

    def listen(self, name="a"):
        path = os.path.join(self.tmp, f"{name}.sock")
        srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        srv.bind(path)
        srv.listen(5)
        self.addCleanup(srv.close)

        def serve():
            while True:
                try:
                    conn, _ = srv.accept()
                except OSError:
                    return
                with conn:
                    buf = b""
                    while True:
                        d = conn.recv(4096)
                        if not d:
                            break
                        buf += d
                    self.received.append(buf)

        threading.Thread(target=serve, daemon=True).start()
        return path

    def register(self, pid, name, sock, file=None, proc_start=None):
        """Session file; procStart defaults to the real start time of the pid."""
        if proc_start is None:
            proc_start = jw.stat_fields(pid)[19]
        with open(os.path.join(self.sessions, f"{file or pid}.json"), "w") as f:
            json.dump({"pid": pid, "name": name, "procStart": proc_start, "messagingSocketPath": sock}, f)

    def dead_pid(self):
        p = subprocess.Popen(["true"])
        p.wait()
        return p.pid

    def spawn(self, cwd=None, env=None):
        """A live process with the given working directory and environment."""
        p = subprocess.Popen(["sleep", "60"], cwd=cwd, env={**os.environ, **(env or {})})
        self.addCleanup(p.wait)
        self.addCleanup(p.kill)
        return p.pid

    def live(self, *entries):
        """Patch jw.live_claudes with (pid, session dict, cwd, shpool name) entries."""
        m = mock.patch.object(jw, "live_claudes", return_value=list(entries))
        m.start()
        self.addCleanup(m.stop)

    def entry(self, pid, name, cwd, sock, shpool=None):
        return (pid, {"pid": pid, "name": name, "messagingSocketPath": sock}, cwd, shpool)

    def msg(self, *args, stdin=""):
        with mock.patch.object(sys, "argv", ["jw", "msg", *args]):
            a = jw.parse_args()
        err = io.StringIO()
        with contextlib.redirect_stderr(err), mock.patch("sys.stdin", io.StringIO(stdin)):
            a.func(a)
        return err.getvalue()

    def sent(self):
        for _ in range(100):
            if self.received:
                break
            time.sleep(0.02)
        self.assertEqual(len(self.received), 1)
        raw = self.received[0].decode()
        self.assertTrue(raw.endswith("\n"))
        self.assertEqual(raw.count("\n"), 1)
        obj = json.loads(raw)
        self.assertEqual(obj["type"], "user")
        self.assertEqual(obj["message"]["role"], "user")
        return obj["message"]["content"]

    def test_match_by_name(self):
        self.register(os.getpid(), "s-1-2-3", self.listen())
        err = self.msg("s-1-2-3", "hello", "there")
        self.assertEqual(self.sent(), "Message from me:\n\n> hello there")
        self.assertIn("jw: sent to s-1-2-3", err)

    def test_match_by_cwd_basename(self):
        self.live(self.entry(1, "s-1-2-3", "/x/repo.agents/s-1-2", self.listen()))
        err = self.msg("s-1-2", "hi")
        self.assertEqual(self.sent(), "Message from me:\n\n> hi")
        self.assertIn("jw: sent to s-1-2-3", err)

    def test_match_by_real_cwd_through_live_claudes(self):
        d = os.path.join(self.tmp, "repo.agents", "ws-7")
        os.makedirs(d)
        pid = self.spawn(cwd=d)
        self.register(pid, "other-name", self.listen())
        err = self.msg("ws-7", "hi")
        self.assertEqual(self.sent(), "Message from me:\n\n> hi")
        self.assertIn("jw: sent to other-name", err)

    def test_match_by_shpool_name(self):
        self.live(self.entry(1, "sess", "/x/unrelated", self.listen(), shpool="pool-9"))
        err = self.msg("pool-9", "hi")
        self.assertEqual(self.sent(), "Message from me:\n\n> hi")
        self.assertIn("jw: sent to sess", err)

    def test_match_by_shpool_name_through_live_claudes(self):
        pid = self.spawn(env={"SHPOOL_SESSION_NAME": "pool-real"})
        self.register(pid, "sess", self.listen())
        self.msg("pool-real", "hi")
        self.assertEqual(self.sent(), "Message from me:\n\n> hi")

    def test_dead_pid_ignored(self):
        self.register(self.dead_pid(), "dead", self.listen(), proc_start="1")
        with self.assertRaises(Fail) as c:
            self.msg("dead", "hi")
        self.assertEqual(str(c.exception), "no running session dead")
        self.assertEqual(self.received, [])

    def test_stale_proc_start_ignored(self):
        self.register(os.getpid(), "stale", self.listen(), proc_start="1")  # live pid, but reused
        with self.assertRaises(Fail) as c:
            self.msg("stale", "hi")
        self.assertEqual(str(c.exception), "no running session stale")
        self.assertEqual(self.received, [])

    def test_dead_pid_does_not_make_ambiguous(self):
        self.register(self.dead_pid(), "dup", self.listen("a"), proc_start="1")
        self.register(os.getpid(), "dup", self.listen("b"), file="alive")
        self.msg("dup", "hi")
        self.assertEqual(self.sent(), "Message from me:\n\n> hi")

    def test_no_match_fails_after_wait(self):
        self.register(os.getpid(), "other", self.listen())
        with self.assertRaises(Fail) as c:
            self.msg("nope", "hi")
        self.assertEqual(str(c.exception), "no running session nope")

    def test_invalid_files_skipped(self):
        with open(os.path.join(self.sessions, "bad.json"), "w") as f:
            f.write("{not json")
        with open(os.path.join(self.sessions, "nosock.json"), "w") as f:
            json.dump({"pid": os.getpid(), "name": "good", "procStart": jw.stat_fields(os.getpid())[19]}, f)
        self.register(os.getpid(), "good", self.listen(), file="ok")
        self.msg("good", "hi")
        self.assertEqual(self.sent(), "Message from me:\n\n> hi")

    def test_ambiguous_fails(self):
        self.live(self.entry(11, "n1", "/x/repo.agents/same", self.listen("a")),
                  self.entry(12, "n2", "/y/same", self.listen("b")))
        with self.assertRaises(Fail) as c:
            self.msg("same", "hi")
        msg = str(c.exception)
        self.assertTrue(msg.startswith("several sessions match same: "), msg)
        self.assertIn("n1 (pid 11)", msg)
        self.assertIn("n2 (pid 12)", msg)
        self.assertEqual(self.received, [])

    def test_multiline_and_control_chars(self):
        self.register(os.getpid(), "s",self.listen())
        text = "a\r\nb\rc\u2028d\u2029e\u0085f\tg\x00h\x1bi\x07j"
        self.msg("s", "--", text)
        self.assertEqual(self.sent(), "Message from me:\n\n> a\n> b\n> c\n> d\n> e\n> f\tghij")

    def test_from_is_sanitized(self):
        self.register(os.getpid(), "s",self.listen())
        self.msg("--from", "ev il\n:/x", "s", "hi")
        self.assertEqual(self.sent(), "Message from evilx:\n\n> hi")

    def test_from_max_64(self):
        self.register(os.getpid(), "s",self.listen())
        self.msg("--from", "a" * 100, "s", "hi")
        self.assertEqual(self.sent(), f"Message from {'a' * 64}:\n\n> hi")

    def test_from_empty_after_sanitizing_is_unknown(self):
        self.register(os.getpid(), "s",self.listen())
        self.msg("--from", "!!! ", "s", "hi")
        self.assertEqual(self.sent(), "Message from unknown:\n\n> hi")

    def test_text_from_stdin(self):
        self.register(os.getpid(), "s",self.listen())
        self.msg("s", stdin="line1\nline2\n")
        self.assertEqual(self.sent(), "Message from me:\n\n> line1\n> line2")

    def test_trailing_line_breaks_stripped(self):
        self.register(os.getpid(), "s", self.listen())
        self.msg("s", "--", "hi\r\n\r\n ")
        self.assertEqual(self.sent(), "Message from me:\n\n> hi")

    def test_long_content_truncated(self):
        self.register(os.getpid(), "s", self.listen())
        self.msg("s", "--", "x" * 9000)
        content = self.sent()
        self.assertTrue(content.startswith("Message from me:\n\n> xxx"))
        self.assertTrue(content.endswith("\n[truncated]"))
        self.assertEqual(len(content), 8000 + len("\n[truncated]"))

    def test_empty_text(self):
        self.register(os.getpid(), "s",self.listen())
        for stdin in (" \n\t\n", ""):
            with self.assertRaises(Fail) as c:
                self.msg("s", stdin=stdin)
            self.assertEqual(str(c.exception), "empty message")
        self.assertEqual(self.received, [])

    def test_event_origin_refused(self):
        self.register(os.getpid(), "s",self.listen())
        with mock.patch.dict(os.environ, {"KNOWLEDGE_ORIGIN": "event:gitlab"}):
            with self.assertRaises(Fail):
                self.msg("s", "hi")
        self.assertEqual(self.received, [])

    def test_unreachable_socket(self):
        self.register(os.getpid(), "s",os.path.join(self.tmp, "gone.sock"))  # nothing listens
        with self.assertRaises(Fail) as c:
            self.msg("s", "hi")
        self.assertTrue(str(c.exception).startswith("cannot reach session s"), str(c.exception))

    def test_session_appearing_during_wait(self):
        sock = self.listen()
        threading.Timer(0.6, self.register, (os.getpid(), "late", sock)).start()
        with mock.patch.object(jw, "MSG_WAIT", 3):
            self.msg("late", "hi")
        self.assertEqual(self.sent(), "Message from me:\n\n> hi")

    def test_socket_listening_during_wait(self):
        path = os.path.join(self.tmp, "later.sock")
        self.register(os.getpid(), "s", path)
        t = threading.Timer(0.6, self.listen, ("later",))
        t.start()
        self.addCleanup(t.join)
        with mock.patch.object(jw, "MSG_WAIT", 3):
            self.msg("s", "hi")
        self.assertEqual(self.sent(), "Message from me:\n\n> hi")

    def test_waits_then_fails_when_nothing_appears(self):
        with mock.patch.object(jw, "MSG_WAIT", 1):
            t0 = time.monotonic()
            with self.assertRaises(Fail) as c:
                self.msg("ghost", "hi")
        self.assertGreaterEqual(time.monotonic() - t0, 0.9)
        self.assertEqual(str(c.exception), "no running session ghost")

    def test_waits_then_fails_when_unreachable(self):
        self.register(os.getpid(), "s", os.path.join(self.tmp, "gone.sock"))
        with mock.patch.object(jw, "MSG_WAIT", 1):
            t0 = time.monotonic()
            with self.assertRaises(Fail) as c:
                self.msg("s", "hi")
        self.assertGreaterEqual(time.monotonic() - t0, 0.9)
        self.assertTrue(str(c.exception).startswith("cannot reach session s"), str(c.exception))


class RecordsDir(unittest.TestCase):
    def test_xdg(self):
        self.assertEqual(jw.records_dir({"XDG_STATE_HOME": "/s", "HOME": "/h"}), "/s/agent-sessions")

    def test_empty_xdg(self):
        self.assertEqual(jw.records_dir({"XDG_STATE_HOME": "", "HOME": "/h"}), "/h/.local/state/agent-sessions")

    def test_relative_xdg(self):
        self.assertEqual(jw.records_dir({"XDG_STATE_HOME": "rel", "HOME": "/h"}),
                         "/h/.local/state/agent-sessions")


class SessionName(unittest.TestCase):
    def test_given(self):
        self.assertEqual(jw.session_name("a-1", {}), "a-1")

    def test_env(self):
        self.assertEqual(jw.session_name(None, {"AGENT_SESSION": "s-1"}), "s-1")

    def test_given_wins(self):
        self.assertEqual(jw.session_name("x", {"AGENT_SESSION": "y"}), "x")

    def test_none(self):
        with self.assertRaises(Fail):
            jw.session_name(None, {})

    def test_invalid(self):
        for n in ["", "../x", ".lock", "-x", "a/b", "x\n"]:
            with self.subTest(name=n), self.assertRaises(Fail):
                jw.session_name(n, {})


class RecordsBase(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(dir="/tmp")
        self.addCleanup(shutil.rmtree, self.tmp, True)
        self.d = os.path.join(self.tmp, "agent-sessions")

    def mkdir(self):
        os.makedirs(self.d, mode=0o700, exist_ok=True)

    def path(self, name):
        return os.path.join(self.d, name)

    def write(self, name, content):
        self.mkdir()
        with open(self.path(name), "wb") as f:
            f.write(content if isinstance(content, bytes) else content.encode())

    def age(self, name, days):
        t = time.time() - days * 86400
        os.utime(self.path(name), (t, t), follow_symlinks=False)


class ReadRecord(RecordsBase):
    def test_valid(self):
        self.write("a.json", '{"kind":"task"}')
        self.assertEqual(jw.read_record(self.d, "a"), {"kind": "task"})

    def test_missing(self):
        self.assertIsNone(jw.read_record(self.d, "a"))
        self.mkdir()
        self.assertIsNone(jw.read_record(self.d, "a"))

    def test_symlink(self):
        self.write("real.json", '{"kind":"task"}')
        os.symlink(self.path("real.json"), self.path("a.json"))
        self.assertIsNone(jw.read_record(self.d, "a"))

    def test_fifo_does_not_block(self):
        self.mkdir()
        os.mkfifo(self.path("a.json"))
        res = []
        t = threading.Thread(target=lambda: res.append(jw.read_record(self.d, "a")), daemon=True)
        t.start()
        t.join(5)
        self.assertFalse(t.is_alive(), "read_record blocked on a FIFO")
        self.assertEqual(res, [None])

    def test_directory(self):
        self.mkdir()
        os.mkdir(self.path("a.json"))
        self.assertIsNone(jw.read_record(self.d, "a"))

    def test_too_big(self):
        body = b'{"a":"' + b"x" * (65537 - 8) + b'"}'
        self.assertEqual(len(body), 65537)
        self.write("a.json", body)
        self.assertIsNone(jw.read_record(self.d, "a"))

    def test_exactly_max(self):
        body = b'{"a":"' + b"x" * (65536 - 8) + b'"}'
        self.assertEqual(len(body), 65536)
        self.write("a.json", body)
        self.assertEqual(jw.read_record(self.d, "a"), {"a": "x" * (65536 - 8)})

    def test_bad_json(self):
        self.write("a.json", "{not json")
        self.assertIsNone(jw.read_record(self.d, "a"))

    def test_bad_utf8(self):
        self.write("a.json", b'{"a":"\xff"}')
        self.assertIsNone(jw.read_record(self.d, "a"))

    def test_non_object(self):
        for body in ["[]", '"s"', "5", "null"]:
            with self.subTest(body=body):
                self.write("a.json", body)
                self.assertIsNone(jw.read_record(self.d, "a"))

    def test_invalid_name(self):
        self.write("x.json", '{"kind":"task"}')
        for n in ["", "../x", ".lock", "-x", "a/b"]:
            with self.subTest(name=n):
                self.assertIsNone(jw.read_record(self.d, n))


class UpdateRecord(RecordsBase):
    def test_create_modes(self):
        r = jw.update_record(self.d, "s-1", {"kind": "task", "title": "T"})
        self.assertEqual(r, {"kind": "task", "title": "T"})
        with open(self.path("s-1.json")) as f:
            self.assertEqual(json.load(f), {"kind": "task", "title": "T"})
        self.assertEqual(os.stat(self.d).st_mode & 0o777, 0o700)
        self.assertEqual(os.stat(self.path("s-1.json")).st_mode & 0o777, 0o600)

    def test_invalid_name_touches_nothing(self):
        with self.assertRaises(Fail):
            jw.update_record(self.d, "../x", {"kind": "task"})
        self.assertFalse(os.path.exists(self.d))
        self.assertFalse(os.path.exists(os.path.join(self.tmp, "x.json")))

    def test_merge_preserves_unknown(self):
        self.write("s.json", json.dumps({"kind": "task", "title": "T", "extra": 1}))
        r = jw.update_record(self.d, "s", {"title": "U"}, ["https://g/1"])
        exp = {"kind": "task", "title": "U", "extra": 1, "mrs": ["https://g/1"]}
        self.assertEqual(r, exp)
        self.assertEqual(jw.read_record(self.d, "s"), exp)

    def test_none_fields_ignored(self):
        self.write("s.json", json.dumps({"kind": "task", "title": "T"}))
        r = jw.update_record(self.d, "s", {"title": None, "kind": "review"})
        self.assertEqual(r, {"kind": "review", "title": "T"})

    def test_mrs_dedupe_order(self):
        r = jw.update_record(self.d, "s", {}, ["https://a", "https://b", "https://a"])
        self.assertEqual(r["mrs"], ["https://a", "https://b"])
        r = jw.update_record(self.d, "s", {}, ["https://c", "https://a"])
        self.assertEqual(r["mrs"], ["https://a", "https://b", "https://c"])

    def test_non_list_mrs_replaced(self):
        self.write("s.json", json.dumps({"mrs": "x"}))
        r = jw.update_record(self.d, "s", {}, ["https://a"])
        self.assertEqual(r["mrs"], ["https://a"])

    def test_corrupt_replaced(self):
        self.write("s.json", "{garbage")
        r = jw.update_record(self.d, "s", {"kind": "task"})
        self.assertEqual(r, {"kind": "task"})
        self.assertEqual(jw.read_record(self.d, "s"), {"kind": "task"})

    def test_raw_utf8(self):
        jw.update_record(self.d, "s", {"title": "ż"})
        with open(self.path("s.json"), "rb") as f:
            raw = f.read()
        self.assertIn("ż".encode(), raw)
        self.assertNotIn(b"\\u", raw)

    def test_prune_on_create_only(self):
        self.write("s.json", "{}")
        self.write("o.json", "{}")
        self.age("o.json", 8)
        self.age("s.json", 8)
        jw.update_record(self.d, "s", {"kind": "x"})  # existing: no prune
        self.assertTrue(os.path.exists(self.path("o.json")))
        self.write("p.json", "{}")
        self.age("p.json", 6)
        jw.update_record(self.d, "new", {"kind": "x"})  # new: prune
        self.assertFalse(os.path.exists(self.path("o.json")))
        self.assertTrue(os.path.exists(self.path("p.json")))
        self.assertTrue(os.path.exists(self.path("new.json")))

    def test_lock_busy(self):
        self.write("s.json", '{"kind":"task"}')
        fd = os.open(self.path(".lock"), os.O_RDWR | os.O_CREAT, 0o600)
        self.addCleanup(os.close, fd)
        fcntl.flock(fd, fcntl.LOCK_EX)
        with mock.patch.object(jw, "LOCK_WAIT", 0.3):
            t0 = time.monotonic()
            with self.assertRaises(Fail) as c:
                jw.update_record(self.d, "s", {"kind": "other"})
        elapsed = time.monotonic() - t0
        self.assertGreaterEqual(elapsed, 0.3)
        self.assertLess(elapsed, 5)
        self.assertIn("busy", str(c.exception))
        with open(self.path("s.json")) as f:
            self.assertEqual(json.load(f), {"kind": "task"})

    def test_lock_released_while_waiting(self):
        self.write("s.json", '{"kind":"task"}')
        fd = os.open(self.path(".lock"), os.O_RDWR | os.O_CREAT, 0o600)
        self.addCleanup(os.close, fd)
        fcntl.flock(fd, fcntl.LOCK_EX)
        timer = threading.Timer(0.1, lambda: fcntl.flock(fd, fcntl.LOCK_UN))
        timer.start()
        self.addCleanup(timer.join)
        with mock.patch.object(jw, "LOCK_WAIT", 2):
            r = jw.update_record(self.d, "s", {"kind": "other"})
        self.assertEqual(r, {"kind": "other"})
        self.assertEqual(jw.read_record(self.d, "s"), {"kind": "other"})

    def test_symlinked_record_not_followed(self):
        self.mkdir()
        outside = os.path.join(self.tmp, "outside.json")
        with open(outside, "w") as f:
            f.write('{"secret":1}')
        os.symlink(outside, self.path("s.json"))
        jw.update_record(self.d, "s", {"kind": "task"})
        with open(outside) as f:
            self.assertEqual(f.read(), '{"secret":1}')
        self.assertFalse(os.path.islink(self.path("s.json")))
        self.assertTrue(os.path.isfile(self.path("s.json")))
        with open(self.path("s.json")) as f:
            self.assertEqual(json.load(f), {"kind": "task"})

    def test_lock_symlink_not_followed(self):
        self.mkdir()
        outside = os.path.join(self.tmp, "outside-lock")
        os.symlink(outside, self.path(".lock"))
        with self.assertRaises((Fail, OSError)):
            jw.update_record(self.d, "s", {"kind": "task"})
        self.assertFalse(os.path.exists(outside))
        self.assertFalse(os.path.exists(self.path("s.json")))


class PruneRecords(RecordsBase):
    def test_deletes_old_regular_only(self):
        self.write("old.json", "{}")
        self.write("new.json", "{}")
        self.write(".lock", "")
        self.write("target.json", "{}")
        os.symlink(self.path("target.json"), self.path("link.json"))
        os.mkfifo(self.path("fifo.json"))
        os.mkdir(self.path("dir.json"))
        for n in ["old.json", ".lock", "link.json", "fifo.json", "dir.json"]:
            self.age(n, 8)
        self.age("target.json", 1)
        self.assertEqual(jw.prune_records(self.d, time.time()), ["old.json"])
        self.assertEqual(sorted(os.listdir(self.d)),
                         [".lock", "dir.json", "fifo.json", "link.json", "new.json", "target.json"])

    def test_sorted(self):
        for n in ["b.json", "a.json"]:
            self.write(n, "{}")
            self.age(n, 9)
        self.assertEqual(jw.prune_records(self.d, time.time()), ["a.json", "b.json"])

    def test_missing_dir(self):
        self.assertEqual(jw.prune_records(self.d, time.time()), [])


class CmdSession(RecordsBase):
    def setUp(self):
        super().setUp()
        self.d = os.path.join(self.tmp, "state", "agent-sessions")
        env = mock.patch.dict(os.environ, {"XDG_STATE_HOME": os.path.join(self.tmp, "state"),
                                           "AGENT_SESSION": "s-1", "KNOWLEDGE_ORIGIN": ""})
        env.start()
        self.addCleanup(env.stop)

    def run_cmd(self, *args):
        with mock.patch.object(sys, "argv", ["jw", "session", *args]):
            a = jw.parse_args()
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            a.func(a)
        return out.getvalue()

    def test_set_creates(self):
        self.assertEqual(self.run_cmd("set", "--kind", "task", "--title", "T"), "")
        with open(self.path("s-1.json")) as f:
            self.assertEqual(json.load(f), {"kind": "task", "title": "T"})
        self.assertEqual(os.stat(self.d).st_mode & 0o777, 0o700)
        self.assertEqual(os.stat(self.path("s-1.json")).st_mode & 0o777, 0o600)

    def test_set_all_fields(self):
        self.run_cmd("set", "--kind", "k", "--description", "D", "--workspace", "/w",
                     "--url", "https://u/1", "--title", "T")
        self.assertEqual(jw.read_record(self.d, "s-1"),
                         {"kind": "k", "description": "D", "workspace": "/w", "url": "https://u/1", "title": "T"})

    def test_set_merge(self):
        self.write("s-1.json", json.dumps({"kind": "task", "title": "T", "extra": 1}))
        self.run_cmd("set", "--title", "U", "--mr", "https://g/1")
        self.assertEqual(jw.read_record(self.d, "s-1"),
                         {"kind": "task", "title": "U", "extra": 1, "mrs": ["https://g/1"]})

    def test_set_mr_dedupe(self):
        self.run_cmd("set", "--mr", "https://a", "--mr", "https://b", "--mr", "https://a")
        self.assertEqual(jw.read_record(self.d, "s-1")["mrs"], ["https://a", "https://b"])

    def test_set_utf8(self):
        self.run_cmd("set", "--title", "ż")
        with open(self.path("s-1.json"), "rb") as f:
            self.assertIn("ż".encode(), f.read())

    def test_set_session_override(self):
        self.run_cmd("set", "-s", "other", "--kind", "k")
        self.assertTrue(os.path.exists(self.path("other.json")))
        self.assertFalse(os.path.exists(self.path("s-1.json")))

    def test_set_no_session(self):
        with mock.patch.dict(os.environ):
            del os.environ["AGENT_SESSION"]
            with self.assertRaises(Fail):
                self.run_cmd("set", "--kind", "k")
        self.assertFalse(os.path.exists(self.d))

    def test_set_refused_in_event(self):
        with mock.patch.dict(os.environ, {"KNOWLEDGE_ORIGIN": "event:gl-mr-1"}):
            with self.assertRaises(Fail):
                self.run_cmd("set", "--kind", "k")
        self.assertFalse(os.path.exists(self.d))
        self.assertFalse(os.path.exists(os.path.dirname(self.d)))

    def test_set_other_origin_allowed(self):
        with mock.patch.dict(os.environ, {"KNOWLEDGE_ORIGIN": "manual"}):
            self.run_cmd("set", "--kind", "k")
        self.assertEqual(jw.read_record(self.d, "s-1"), {"kind": "k"})

    def test_set_bad_urls(self):
        bad = ["http://x", "javascript:x", "https://a b", "https://", "https://" + "a" * 2041]
        self.assertEqual(len(bad[-1]), 2049)
        for u in bad:
            for flag in ("--url", "--mr"):
                with self.subTest(url=u[:30], flag=flag):
                    with self.assertRaises(Fail):
                        self.run_cmd("set", "--kind", "k", flag, u)
                    self.assertFalse(os.path.exists(self.d))

    def test_set_bad_chars_in_urls(self):
        bad = ["https://x\x1b]0;e\x07", "https://x‮", "https://ż"]
        for u in bad:
            for flag in ("--url", "--mr"):
                with self.subTest(url=repr(u), flag=flag):
                    with self.assertRaises(Fail):
                        self.run_cmd("set", "--kind", "k", flag, u)
                    self.assertFalse(os.path.exists(self.d))

    def test_set_url_printable_ascii_ok(self):
        u = "https://g/a-b_c?x=1&y=%20"
        self.run_cmd("set", "--url", u, "--mr", u)
        r = jw.read_record(self.d, "s-1")
        self.assertEqual(r["url"], u)
        self.assertEqual(r["mrs"], [u])

    def test_invalid_name_cli(self):
        with self.assertRaises(Fail):
            self.run_cmd("get", "-s", "../x")
        with self.assertRaises(Fail):
            self.run_cmd("set", "-s", "../x", "--title", "t")
        self.assertFalse(os.path.exists(self.d))
        self.assertFalse(os.path.exists(os.path.join(self.tmp, "state", "x.json")))
        self.assertFalse(os.path.exists(os.path.join(self.tmp, "x.json")))

    def test_set_bad_url_leaves_existing(self):
        self.write("s-1.json", '{"kind":"task"}')
        with self.assertRaises(Fail):
            self.run_cmd("set", "--kind", "k", "--mr", "https://ok", "--mr", "http://bad")
        self.assertEqual(jw.read_record(self.d, "s-1"), {"kind": "task"})

    def test_set_url_max_ok(self):
        u = "https://" + "a" * 2040
        self.assertEqual(len(u), 2048)
        self.run_cmd("set", "--url", u)
        self.assertEqual(jw.read_record(self.d, "s-1")["url"], u)

    def test_set_surrogate_title(self):
        try:
            self.run_cmd("set", "--title", BAD)
        except Fail as e:
            self.assertIn("cannot write the session record", str(e))
            self.assertIsNone(jw.read_record(self.d, "s-1"))
        else:
            self.assertEqual(jw.read_record(self.d, "s-1")["title"], "fix \ufffd x")
        self.assertFalse(os.path.exists(self.path("s-1.json")) and jw.read_record(self.d, "s-1") is None)
        self.assertEqual([n for n in os.listdir(self.d) if n.startswith(".tmp-")], [])

    def test_get(self):
        self.write("s-1.json", json.dumps({"kind": "task", "title": "ż"}, ensure_ascii=False))
        out = self.run_cmd("get")
        self.assertTrue(out.endswith("\n"))
        self.assertEqual(out.count("\n"), 1)
        self.assertEqual(json.loads(out), {"kind": "task", "title": "ż"})
        self.assertIn("ż", out)

    def test_get_override(self):
        self.write("o.json", '{"kind":"x"}')
        self.assertEqual(json.loads(self.run_cmd("get", "-s", "o")), {"kind": "x"})

    def test_get_missing(self):
        with self.assertRaises(Fail) as c:
            self.run_cmd("get")
        self.assertIn("no session record s-1", str(c.exception))

    def test_get_corrupt(self):
        self.write("s-1.json", "{bad")
        with self.assertRaises(Fail):
            self.run_cmd("get")


class ProjectKey(unittest.TestCase):
    def test_short(self):
        self.assertEqual(jw.project_key("/home/nm1/projects/home-infra"), "-home-nm1-projects-home-infra")

    def test_non_alphanumerics(self):
        self.assertEqual(jw.project_key("/a/b.c_d e"), "-a-b-c-d-e")

    def test_exactly_64_kept(self):
        p = "/" + "a" * 63
        self.assertEqual(jw.project_key(p), "-" + "a" * 63)

    def test_uses_cksum_constant(self):
        p = "/home/nm1/projects/" + "x" * 80
        with mock.patch.object(jw, "CKSUM", "/nonexistent/cksum"):
            with self.assertRaises(OSError):
                jw.project_key(p)

    def test_long_gets_tail_and_cksum(self):
        p = "/home/nm1/projects/some-really-long-repository-name-that-goes-beyond-sixty-four-chars"
        crc = subprocess.run(["cksum"], input=p.encode(), stdout=subprocess.PIPE).stdout.split()[0].decode()
        self.assertEqual(crc, "1770022754")
        dashed = "".join(c if c.isalnum() else "-" for c in p)
        self.assertEqual(jw.project_key(p), f"{dashed[-50:]}-{crc}")


class WorkspaceRoot(unittest.TestCase):
    def test_agent_workspace(self):
        self.assertEqual(jw.workspace_root("/h/r.agents/s-1/clusters/x", "/h/r"), "/h/r.agents/s-1")
        self.assertEqual(jw.workspace_root("/h/r.agents/s-1", "/h/r"), "/h/r.agents/s-1")

    def test_home(self):
        self.assertEqual(jw.workspace_root("/h/r", "/h/r"), "/h/r")
        self.assertEqual(jw.workspace_root("/h/r/pkgs/jw", "/h/r"), "/h/r")

    def test_outside(self):
        for cwd in ("/h/other", "/h/r2", "/h/r2/x", "/h/r.agents/", "/h/other.agents/s-1", "/tmp"):
            with self.subTest(cwd=cwd):
                self.assertIsNone(jw.workspace_root(cwd, "/h/r"))


HOME = "/r"
A = "/r.agents/a"
B = "/r.agents/b"


class Transcript(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.path = os.path.join(tmp.name, "t.jsonl")

    def write(self, *entries):
        with open(self.path, "w") as f:
            for e in entries:
                f.write(e if isinstance(e, str) else json.dumps(e))
                f.write("\n")
        return jw.transcript(self.path, HOME)

    @staticmethod
    def user(text, **kw):
        return {"type": "user", "message": {"role": "user", "content": text}, **kw}

    def test_first_user_message_and_last_cwd(self):
        r = self.write(self.user("hello", cwd=A), self.user("second", cwd=B))
        self.assertEqual(r, (B, "hello"))

    def test_skips_non_json_lines(self):
        r = self.write("not json {", self.user("hi", cwd=A), "", "][")
        self.assertEqual(r, (A, "hi"))

    def test_collapses_and_truncates(self):
        r = self.write(self.user("a  b\n\n c\t" + "x" * 200, cwd=A))
        self.assertEqual(r[0], A)
        self.assertTrue(r[1].startswith("a b c x"))
        self.assertNotIn("\n", r[1])
        self.assertEqual(len(r[1]), 100)

    def test_ai_title_beats_message(self):
        r = self.write(self.user("hi", cwd=A), {"type": "ai-title", "aiTitle": "old"},
                       {"type": "ai-title", "aiTitle": "new"})
        self.assertEqual(r, (A, "new"))

    def test_custom_title_beats_ai_title(self):
        r = self.write(self.user("hi", cwd=A), {"type": "custom-title", "customTitle": "c1"},
                       {"type": "ai-title", "aiTitle": "ai"}, {"type": "custom-title", "customTitle": "c2"})
        self.assertEqual(r, (A, "c2"))

    def test_tool_results_empty_and_meta_are_not_real(self):
        tool = {"type": "user", "message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "t", "content": "out"}]}}
        r = self.write(tool, self.user("", cwd=A), self.user("meta", isMeta=True),
                       self.user("real", cwd=A))
        self.assertEqual(r, (A, "real"))

    def test_text_blocks(self):
        e = {"type": "user", "cwd": A, "message": {"role": "user", "content": [{"type": "text", "text": "blocky"}]}}
        self.assertEqual(self.write(e), (A, "blocky"))

    def test_none_without_user_message(self):
        self.assertIsNone(self.write({"type": "ai-title", "aiTitle": "t", "cwd": A}))

    def test_none_without_real_user_message(self):
        self.assertIsNone(self.write(self.user("meta", isMeta=True, cwd=A)))

    def test_none_without_cwd(self):
        self.assertIsNone(self.write(self.user("hi")))

    def test_last_workspace_cwd_wins_over_outside_dir(self):
        r = self.write(self.user("hi", cwd=A), self.user("more", cwd=B), self.user("x", cwd="/elsewhere/dir"))
        self.assertEqual(r, (B, "hi"))

    def test_subdirectory_maps_to_workspace_root(self):
        self.assertEqual(self.write(self.user("hi", cwd=A + "/sub/dir")), (A, "hi"))

    def test_none_when_no_cwd_in_repo(self):
        self.assertIsNone(self.write(self.user("hi", cwd="/elsewhere")))


class CmdResume(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.tmp = tmp.name
        self.home = os.path.join(self.tmp, "projects", "repo")
        os.makedirs(self.home)
        self.cfg = os.path.join(self.tmp, "claude")
        self.proj = os.path.join(self.cfg, "projects", jw.project_key(self.home))
        os.makedirs(self.proj)
        self.cwd = os.path.join(self.tmp, "projects", "repo.agents", "w1")
        os.makedirs(self.cwd)
        self.live = []

    def session(self, sid, mtime, text="task", cwd=None):
        p = os.path.join(self.proj, sid + ".jsonl")
        with open(p, "w") as f:
            f.write(json.dumps({"type": "user", "cwd": cwd or self.cwd,
                                "message": {"role": "user", "content": text}}) + "\n")
        os.utime(p, (mtime, mtime))

    def resume(self, last=False, picked=None, repo=None):
        self.err = io.StringIO()
        self.picker = mock.Mock(side_effect=lambda rows, *a, **kw: rows[picked if picked is not None else 0])
        a = mock.Mock(repo=repo, detach=False, last=last)
        with contextlib.ExitStack() as st:
            st.enter_context(mock.patch.dict(os.environ, {"CLAUDE_CONFIG_DIR": self.cfg}))
            st.enter_context(mock.patch.object(jw, "locate", return_value=("x", self.home)))
            st.enter_context(mock.patch.object(jw, "repo_home", return_value=self.home))
            st.enter_context(mock.patch.object(jw, "live_claudes", return_value=self.live))
            st.enter_context(mock.patch.object(jw, "pick", self.picker))
            self.add_ws = st.enter_context(mock.patch.object(jw, "add_workspace", return_value="/new/ws"))
            self.fetch = st.enter_context(mock.patch.object(jw, "fetch"))
            self.launch = st.enter_context(mock.patch.object(jw, "launch"))
            st.enter_context(mock.patch("sys.stderr", self.err))
            jw.cmd_resume(a)

    def preferred(self):
        c = self.launch.call_args
        return c.kwargs["preferred"] if "preferred" in c.kwargs else c.args[5]

    def test_last_takes_newest_without_picker(self):
        self.session("old-1", 1000, "old")
        self.session("new-2", 3000, "new")
        self.session("mid-3", 2000, "mid")
        self.resume(last=True)
        self.picker.assert_not_called()
        home, detach, argv, text, make_dir = self.launch.call_args.args[:5]
        self.assertEqual(home, self.home)
        self.assertFalse(detach)
        self.assertEqual(argv, ["claude", "--resume", "new-2"])
        self.assertEqual(text, "new")
        self.assertEqual(self.preferred(), "w1")

    def test_picker_rows_newest_first(self):
        self.session("old-1", 1000, "old")
        self.session("new-2", 3000, "new")
        self.session("mid-3", 2000, "mid")
        self.resume()
        rows = self.picker.call_args.args[0]
        self.assertEqual(len(rows), 3)
        for row, title in zip(rows, ["new", "mid", "old"]):
            self.assertIn(title, row)

    def test_picked_row_is_resumed(self):
        self.session("old-1", 1000, "old")
        self.session("new-2", 3000, "new")
        self.resume(picked=1)
        self.assertEqual(self.launch.call_args.args[2], ["claude", "--resume", "old-1"])
        self.assertEqual(self.launch.call_args.args[3], "old")

    def test_live_sessions_skipped(self):
        self.session("new-2", 3000, "new")
        self.session("old-1", 1000, "old")
        self.live = [(1, {"sessionId": "new-2"}, self.cwd, None)]
        self.resume(last=True)
        self.assertEqual(self.launch.call_args.args[2], ["claude", "--resume", "old-1"])
        self.resume()
        self.assertEqual(len(self.picker.call_args.args[0]), 1)

    def test_unusable_transcripts_skipped(self):
        self.session("good-1", 1000, "good")
        with open(os.path.join(self.proj, "bad-2.jsonl"), "w") as f:
            f.write("{}\n")
        os.utime(os.path.join(self.proj, "bad-2.jsonl"), (5000, 5000))
        self.resume(last=True)
        self.assertEqual(self.launch.call_args.args[2], ["claude", "--resume", "good-1"])

    def test_no_rows_fails(self):
        with self.assertRaises(Fail):
            self.resume(last=True)
        self.launch.assert_not_called()

    def test_all_live_fails(self):
        self.session("a-1", 1000)
        self.live = [(1, {"sessionId": "a-1"}, self.cwd, None)]
        with self.assertRaises(Fail):
            self.resume()

    def test_no_project_dir_fails(self):
        shutil.rmtree(self.proj)
        with self.assertRaises(Fail):
            self.resume(last=True)

    def test_make_dir_existing_cwd(self):
        self.session("a-1", 1000)
        self.resume(last=True)
        make_dir = self.launch.call_args.args[4]
        with mock.patch.object(jw, "add_workspace") as add_ws:
            self.assertEqual(make_dir("w1"), self.cwd)
        add_ws.assert_not_called()

    def test_make_dir_recreates_missing_cwd(self):
        gone = os.path.join(self.tmp, "projects", "repo.agents", "gone")
        self.session("a-1", 1000, cwd=gone)
        self.resume(last=True)
        self.assertEqual(self.preferred(), "gone")
        make_dir = self.launch.call_args.args[4]
        calls = mock.Mock()
        with mock.patch.object(jw, "add_workspace", return_value="/new/ws") as add_ws, \
                mock.patch.object(jw, "fetch") as fetch, mock.patch("sys.stderr", self.err):
            calls.attach_mock(fetch, "fetch")
            calls.attach_mock(add_ws, "add")
            self.assertEqual(make_dir("gone"), "/new/ws")
        self.assertEqual(calls.mock_calls, [mock.call.fetch(self.home),
                                            mock.call.add(self.home, "trunk()", "gone", None)])
        self.assertIn(gone, self.err.getvalue())

    def test_subdirectory_cwd_uses_workspace_root(self):
        self.session("a-1", 1000, cwd=os.path.join(self.cwd, "clusters", "x"))
        self.resume(last=True)
        self.assertEqual(self.preferred(), "w1")
        self.assertEqual(self.launch.call_args.args[4]("w1"), self.cwd)

    def test_deleted_subdirectory_recreates_by_root_name(self):
        gone = os.path.join(self.tmp, "projects", "repo.agents", "gone", "sub")
        self.session("a-1", 1000, cwd=gone)
        self.resume(last=True)
        self.assertEqual(self.preferred(), "gone")
        with mock.patch.object(jw, "add_workspace", return_value="/new/ws") as add_ws, \
                mock.patch.object(jw, "fetch"), mock.patch("sys.stderr", self.err):
            self.assertEqual(self.launch.call_args.args[4]("gone"), "/new/ws")
        add_ws.assert_called_once_with(self.home, "trunk()", "gone", None)

    def test_home_cwd_resumed_in_home(self):
        self.session("a-1", 1000, cwd=self.home)
        self.resume(last=True)
        self.assertEqual(self.launch.call_args.args[4]("x"), self.home)

    def test_outside_cwd_skipped(self):
        self.session("a-1", 1000, cwd="/elsewhere")
        with self.assertRaises(Fail):
            self.resume(last=True)

    def test_event_session_fails(self):
        self.session("a-1", 1000)
        with mock.patch.dict(os.environ, {"KNOWLEDGE_ORIGIN": "event:comment"}):
            with self.assertRaises(Fail):
                self.resume(last=True)
        self.launch.assert_not_called()

    def test_gitlab_workspaces_skipped(self):
        gl = os.path.join(self.tmp, "projects", "repo.agents", "gl-mr-12-3")
        os.makedirs(gl)
        self.session("gl-1", 3000, cwd=gl)
        self.session("old-1", 1000)
        self.resume(last=True)
        self.assertEqual(self.launch.call_args.args[2], ["claude", "--resume", "old-1"])
        self.resume()
        self.assertEqual(len(self.picker.call_args.args[0]), 1)

    def test_last_stops_reading_older_transcripts(self):
        self.session("old-1", 1000)
        self.session("new-2", 3000)
        with mock.patch.object(jw, "transcript", wraps=jw.transcript) as t:
            self.resume(last=True)
        self.assertEqual(t.call_count, 1)
        self.assertTrue(t.call_args.args[0].endswith("new-2.jsonl"))

    def test_repo_argument_used(self):
        self.session("a-1", 1000)
        with mock.patch.object(jw, "locate", side_effect=Fail("no")):
            self.resume(last=True, repo="repo")
        self.assertEqual(self.launch.call_args.args[0], self.home)


class Launch(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.tmp = tmp.name
        self.home = os.path.join(self.tmp, "repo")
        os.makedirs(self.home)
        self.run_dir = os.path.join(self.tmp, "run")
        os.makedirs(self.run_dir)

    def go(self, taken, preferred, detach=True, resume=False):
        self.job = None

        def run(argv, *a, **kw):
            self.job = open(argv[argv.index("-c") + 1].split()[-1]).read()
            self.argv = argv
            return mock.Mock(returncode=0)

        names = []
        err = io.StringIO()
        with contextlib.ExitStack() as st:
            st.enter_context(mock.patch.dict(os.environ, {"XDG_RUNTIME_DIR": self.run_dir,
                                                           "XDG_STATE_HOME": os.path.join(self.tmp, "state")}))
            st.enter_context(mock.patch.object(jw, "shpool_status", return_value={n: "detached" for n in taken}))
            st.enter_context(mock.patch.object(jw.time, "strftime", return_value="s-1"))
            st.enter_context(mock.patch.object(jw, "attach"))
            st.enter_context(mock.patch("subprocess.run", side_effect=run))
            st.enter_context(mock.patch("sys.stderr", err))
            make_dir = mock.Mock(side_effect=lambda n: names.append(n) or self.tmp)
            jw.launch(self.home, detach, ["claude", "--resume", "sid"], "text", make_dir, preferred=preferred, resume=resume)
        return names[0], err.getvalue()

    def test_preferred_used(self):
        name, err = self.go([], "w1")
        self.assertEqual(name, "w1")
        self.assertIn("export SHPOOL_SESSION_NAME=w1\n", self.job)
        self.assertIn("exec claude --resume sid\n", self.job)
        self.assertEqual(self.argv[-1], "w1")
        self.assertIn("jw attach w1", err)

    def test_preferred_taken_falls_back(self):
        name, _ = self.go(["w1"], "w1")
        self.assertEqual(name, "s-1")

    def test_preferred_invalid_falls_back(self):
        for bad in ("bad name", "a/b", ""):
            with self.subTest(bad=bad):
                name, _ = self.go([], bad)
                self.assertEqual(name, "s-1")

    def state(self):
        return mock.patch.dict(os.environ, {"XDG_STATE_HOME": os.path.join(self.tmp, "state")})

    def seed(self):
        with self.state():
            jw.update_record(jw.records_dir(), "w1", {"kind": "task", "workspace": "/old", "title": "orig"})

    def record(self):
        with self.state():
            return jw.read_record(jw.records_dir(), "w1")

    def test_resume_existing_record_only_updates_workspace(self):
        self.seed()
        self.go([], "w1", resume=True)
        r = self.record()
        self.assertEqual(r["workspace"], self.tmp)
        self.assertEqual(r["title"], "orig")

    def test_resume_without_record_writes_full_record(self):
        self.go([], "w1", resume=True)
        r = self.record()
        for k, v in jw.task_record("text", self.tmp).items():
            self.assertEqual(r[k], v)

    def test_no_resume_writes_full_record_over_existing(self):
        self.seed()
        self.go([], "w1")
        self.assertEqual(self.record()["title"], "text")

    def test_no_preferred_uniquifies(self):
        name, _ = self.go(["s-1"], None)
        self.assertEqual(name, "s-1-2")


class CliResume(unittest.TestCase):
    def parse(self, *args):
        with mock.patch.object(sys, "argv", ["jw", *args]):
            return jw.parse_args()

    def test_defaults(self):
        a = self.parse("resume")
        self.assertIs(a.func, jw.cmd_resume)
        self.assertFalse(a.detach)
        self.assertFalse(a.last)
        self.assertFalse(a.repo)

    def test_alias_and_flags(self):
        for flag in ("-l", "--last"):
            a = self.parse("r", "-d", flag, "foo")
            self.assertIs(a.func, jw.cmd_resume)
            self.assertTrue(a.detach)
            self.assertTrue(a.last)
            self.assertEqual(a.repo, "foo")


if __name__ == "__main__":
    unittest.main()
