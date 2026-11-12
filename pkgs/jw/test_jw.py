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

    def claude_argv(self, args):
        with mock.patch.object(jw, "launch") as launch, mock.patch.object(jw, "fetch"), \
                mock.patch.object(jw, "locate", return_value=("x", self.tmp)), \
                mock.patch.object(jw, "repos", return_value=[]):
            jw.cmd_agent(mock.Mock(args=args, revision=None, detach=True))
        return launch.call_args.args[2]

    def test_prompt_follows_double_dash(self):
        self.assertEqual(self.claude_argv(["--", "fix x"]), ["claude", "--", "fix x"])

    def test_dash_prompt_is_not_an_option(self):
        self.assertEqual(self.claude_argv(["--", "--settings={}"]), ["claude", "--", "--settings={}"])

    def test_no_prompt_has_no_double_dash(self):
        self.assertEqual(self.claude_argv([]), ["claude"])

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

    def test_initial_record_resets_finished(self):
        jw.update_record(os.path.join(self.state, "agent-sessions"), "s-1",
                         {"kind": "old", "status": "done", "status_at": 1, "waiting_for": "q",
                          "notified": 1, "reaped": 2})
        self.agent()
        r = json.loads(self.at_attach)
        for k in ("status", "status_at", "waiting_for", "notified", "reaped", "done"):
            self.assertNotIn(k, r)
        self.assertEqual(r["workspace"], self.work)

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
            mock.patch.object(jw, "load_records", return_value=[]),
            mock.patch.object(jw.shutil, "which", return_value="/bin/x"),
        ]
        with contextlib.ExitStack() as st:
            m = [st.enter_context(p) for p in patches]
            tmp = st.enter_context(tempfile.TemporaryDirectory(dir="/tmp"))
            st.enter_context(mock.patch.dict(os.environ, {"XDG_RUNTIME_DIR": tmp, "XDG_STATE_HOME": tmp}))
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


class AgentWorkspace(unittest.TestCase):
    def setUp(self):
        self.tmp = os.path.realpath(tempfile.mkdtemp(dir="/tmp"))
        self.addCleanup(shutil.rmtree, self.tmp, True)
        self.root = os.path.join(self.tmp, "root")
        os.mkdir(self.root)
        p = mock.patch.object(jw, "ROOT", self.root)
        p.start()
        self.addCleanup(p.stop)

    def make(self, repo="r", name="w", repo_jj=True, ws_jj=True):
        os.makedirs(f"{self.root}/{repo}", exist_ok=True)
        if repo_jj:
            os.makedirs(f"{self.root}/{repo}/.jj", exist_ok=True)
        os.makedirs(f"{self.root}/{repo}.agents/{name}", exist_ok=True)
        if ws_jj:
            os.makedirs(f"{self.root}/{repo}.agents/{name}/.jj", exist_ok=True)
        return f"{self.root}/{repo}.agents/{name}"

    def expect(self, repo="r", name="w"):
        return (f"{self.root}/{repo}", name, f"{self.root}/{repo}.agents/{name}")

    def test_valid(self):
        ws = self.make()
        self.assertEqual(jw.agent_workspace(ws), self.expect())

    def test_valid_other_names(self):
        ws = self.make("my-repo", "s-1.x_y")
        self.assertEqual(jw.agent_workspace(ws), self.expect("my-repo", "s-1.x_y"))

    def test_missing(self):
        self.make()
        self.assertIsNone(jw.agent_workspace(f"{self.root}/r.agents/nope"))

    def test_relative(self):
        self.make()
        old = os.getcwd()
        self.addCleanup(os.chdir, old)
        os.chdir(self.root)
        self.assertIsNone(jw.agent_workspace("r.agents/w"))

    def test_not_under_agents(self):
        self.make()
        os.makedirs(f"{self.root}/r/sub/.jj")
        self.assertIsNone(jw.agent_workspace(f"{self.root}/r/sub"))
        self.assertIsNone(jw.agent_workspace(f"{self.root}/r"))

    def test_agents_not_directly_in_root(self):
        os.makedirs(f"{self.root}/x/r/.jj")
        os.makedirs(f"{self.root}/x/r.agents/w/.jj")
        self.assertIsNone(jw.agent_workspace(f"{self.root}/x/r.agents/w"))

    def test_outside_root(self):
        other = f"{self.tmp}/other"
        os.makedirs(f"{other}/r/.jj")
        os.makedirs(f"{other}/r.agents/w/.jj")
        self.assertIsNone(jw.agent_workspace(f"{other}/r.agents/w"))

    def test_nested_deeper(self):
        ws = self.make()
        os.makedirs(f"{ws}/sub/.jj")
        self.assertIsNone(jw.agent_workspace(f"{ws}/sub"))

    def test_repo_without_jj(self):
        ws = self.make(repo_jj=False)
        self.assertIsNone(jw.agent_workspace(ws))

    def test_bad_names(self):
        for name in (".hidden", "-x", "a b", "a:b"):
            ws = self.make(name=name)
            self.assertIsNone(jw.agent_workspace(ws), name)

    def test_workspace_without_jj(self):
        ws = self.make(ws_jj=False)
        self.assertIsNone(jw.agent_workspace(ws))

    def test_jj_is_a_file(self):
        ws = self.make(ws_jj=False)
        open(f"{ws}/.jj", "w").close()
        self.assertIsNone(jw.agent_workspace(ws))

    def test_not_a_str(self):
        self.make()
        for v in (None, 5, b"x", ["a"]):
            self.assertIsNone(jw.agent_workspace(v), v)

    def test_symlinked_root(self):
        ws = self.make()
        link = f"{self.tmp}/link"
        os.symlink(self.root, link)
        expect = (f"{link}/r", "w", f"{link}/r.agents/w")  # built from ROOT, like layout/remove
        with mock.patch.object(jw, "ROOT", link):
            self.assertEqual(jw.agent_workspace(f"{link}/r.agents/w"), expect)
            self.assertEqual(jw.agent_workspace(ws), expect)

    def test_nul_in_path(self):
        self.make()
        self.assertIsNone(jw.agent_workspace(f"{self.root}/r.agents/w\0x"))
        self.assertIsNone(jw.agent_workspace("\0"))

    def test_symlinked_workspace_leaving_root(self):
        self.make()
        other = f"{self.tmp}/other/w"
        os.makedirs(f"{other}/.jj")
        os.symlink(other, f"{self.root}/r.agents/l")
        self.assertIsNone(jw.agent_workspace(f"{self.root}/r.agents/l"))


class LoadRecords(unittest.TestCase):
    def setUp(self):
        self.d = tempfile.mkdtemp(dir="/tmp")
        self.addCleanup(shutil.rmtree, self.d, True)

    def write(self, name, content):
        with open(os.path.join(self.d, name), "w") as f:
            f.write(content)

    def test_missing_dir(self):
        self.assertEqual(jw.load_records(self.d + "/nope"), [])

    def test_filters_and_sorts(self):
        self.write("b.json", '{"x": 2}')
        self.write("a.json", '{"x": 1}')
        self.write("list.json", "[1]")
        self.write("str.json", '"s"')
        self.write("bad.json", "{nope")
        self.write("bad name.json", "{}")
        self.write(".hidden.json", "{}")
        self.write("-dash.json", "{}")
        self.write("c.txt", "{}")
        self.write("d", "{}")
        self.assertEqual(jw.load_records(self.d), [("a", {"x": 1}), ("b", {"x": 2})])

    def test_empty_dir(self):
        self.assertEqual(jw.load_records(self.d), [])

    def test_name_with_dots(self):
        self.write("s-1.x.json", "{}")
        self.assertEqual(jw.load_records(self.d), [("s-1.x", {})])


class CmdReapRecords(unittest.TestCase):
    """cmd_reap's record loop with every helper mocked, a temp ROOT, XDG_STATE_HOME and XDG_RUNTIME_DIR."""

    def setUp(self):
        self.tmp = os.path.realpath(tempfile.mkdtemp(dir="/tmp"))
        self.addCleanup(shutil.rmtree, self.tmp, True)
        self.root = f"{self.tmp}/root"
        self.rt = f"{self.tmp}/run"
        self.state = f"{self.tmp}/state"
        for d in (self.root, self.rt, self.state):
            os.mkdir(d)
        os.makedirs(f"{self.root}/r/.jj")
        self.home = f"{self.root}/r"
        self.calls = []
        self.out, self.err = io.StringIO(), io.StringIO()
        self.sessions = {}
        self.missing = None
        self.idle = []
        self.live = ["live"]
        self.runs = []
        self.m = {}

        def rec(name, ret=None, exc=None):
            def f(*a, **k):
                self.calls.append(name)
                if exc is not None:
                    raise exc
                return ret
            return f

        self.rec = rec
        self.set("stop_sessions", [])
        self.set("retire", [])
        self.set("stack_merged", True)
        self.set("deliver", "label")

        self.cs_args = []

        def claude_sessions(d):
            self.calls.append("claude_sessions")
            self.cs_args.append(d)
            return self.sessions

        def live():
            self.calls.append("live_claudes")
            return self.live

        def idle(claudes, state, now, *a, **k):
            self.calls.append("idle_sessions")
            self.idle_args = (claudes, state)
            return self.idle

        def run(cmd, *a, **k):
            self.calls.append("subprocess")
            self.runs.append(cmd)
            return mock.Mock(returncode=0)

        stack = contextlib.ExitStack()
        self.addCleanup(stack.close)
        for p in (
            mock.patch.object(jw, "ROOT", self.root),
            mock.patch.dict(os.environ, {"XDG_STATE_HOME": self.state, "XDG_RUNTIME_DIR": self.rt}),
            mock.patch.object(jw, "claude_sessions", claude_sessions),
            mock.patch.object(jw, "live_claudes", live),
            mock.patch.object(jw, "shpool_status", rec("shpool_status", {"s": "x"})),
            mock.patch.object(jw, "idle_sessions", idle),
            mock.patch.object(jw.shutil, "which", lambda t: None if t == self.missing else "/bin/" + t),
            mock.patch("subprocess.run", run),
            mock.patch("sys.stdout", self.out),
            mock.patch("sys.stderr", self.err),
        ):
            stack.enter_context(p)

    def set(self, name, ret=None, exc=None):
        p = mock.Mock(side_effect=self.rec(name, ret, exc))
        self.m[name] = p
        patcher = mock.patch.object(jw, name, p)
        patcher.start()
        self.addCleanup(patcher.stop)

    def ws(self, name="w", jj=True):
        d = f"{self.root}/r.agents/{name}"
        os.makedirs(f"{d}/.jj" if jj else d, exist_ok=True)
        return d

    def record(self, name="w", ws=True, **fields):
        if ws is True:
            ws = self.ws(name)
        if ws:
            fields["workspace"] = ws
        jw.update_record(f"{self.state}/agent-sessions", name, fields)

    def read(self, name="w"):
        return jw.read_record(f"{self.state}/agent-sessions", name)

    def reap(self):
        try:
            jw.cmd_reap(mock.Mock())
            self.exc = None
        except BaseException as e:
            self.exc = e
        return self.out.getvalue(), self.err.getvalue()

    def path(self, name="w"):
        return f"{self.root}/r.agents/{name}"

    def helpers(self):
        return [c for c in self.calls if c not in ("live_claudes", "shpool_status", "idle_sessions")]

    def live_session(self):
        self.sessions = {1: ("idle", "sh", 1, "sid")}

    # skips

    def test_no_records(self):
        self.reap()
        self.assertIsNone(self.exc)
        self.assertEqual(self.helpers(), [])

    def test_reaped_skipped_without_calls(self):
        self.record(status="done", reaped=True)
        self.record("m", mrs=["https://x/1"], reaped=True)
        self.reap()
        self.assertIsNone(self.exc)
        self.assertEqual(self.helpers(), [])

    def test_bad_workspace_skipped(self):
        self.record("a", ws=f"{self.root}/nowhere", status="done")
        self.record("b", ws=False, status="done")
        self.record("c", ws=self.ws("c", jj=False), status="done")
        self.record("d", ws="rel/path", mrs=["https://x/1"])
        self.reap()
        self.assertIsNone(self.exc)
        self.assertEqual(self.helpers(), [])
        for n in "abcd":
            self.assertNotIn("reaped", self.read(n))

    def test_neither_done_nor_mrs_skipped(self):
        self.record("a")
        self.record("b", status="busy")
        self.record("c", mrs=[])
        self.record("d", done=True)
        self.reap()
        self.assertEqual(self.helpers(), [])

    # done path

    def test_done_stops_then_retires_and_marks(self):
        self.set("stop_sessions", [("sh", "sid1")])
        self.record(status="done")
        out, err = self.reap()
        self.assertIsNone(self.exc)
        self.assertEqual(self.helpers(), ["stop_sessions", "retire"])
        self.m["stop_sessions"].assert_called_once_with(self.path(), 600)
        self.m["retire"].assert_called_once_with(self.home, "w", yes=True, idle=600)
        self.assertTrue(self.read()["reaped"])
        self.assertIn(f"stopped sh in {self.path()}: AGENT_SESSION=sh claude --resume sid1\n", out)
        self.assertIn("removed r.agents/w", out)

    def test_done_stop_fail_no_retire_no_mark(self):
        self.set("stop_sessions", exc=Fail("busy"))
        self.record(status="done")
        self.reap()
        self.assertEqual(self.helpers(), ["stop_sessions"])
        self.assertNotIn("reaped", self.read())

    def test_done_retire_fail_kept_and_marked(self):
        self.set("retire", exc=Fail("unmerged work"))
        self.record(status="done")
        out, err = self.reap()
        self.assertIn(f"kept {self.path()}: unmerged work", out)
        self.assertNotIn("removed", out)
        self.assertTrue(self.read()["reaped"])

    def test_done_retire_unchecked_kept_and_marked(self):
        self.set("retire", exc=jw.Unchecked("couldn't check the stack"))
        self.record(status="done")
        out, err = self.reap()
        self.assertIn(f"kept {self.path()}: couldn't check the stack", out)
        self.assertNotIn("removed", out)
        self.assertTrue(self.read()["reaped"])

    def test_done_gitlab_workspace_not_retired(self):
        self.record("gl-ab-12", status="done")
        self.reap()
        self.assertNotIn("retire", self.calls)
        self.m["stop_sessions"].assert_called_once_with(self.path("gl-ab-12"), 600)
        self.assertTrue(self.read("gl-ab-12")["reaped"])

    def killed_exc(self, cls, *args):
        e = cls(*args)
        e.killed = [("s", "sid")]
        return e

    def test_done_failing_retire_reports_killed(self):
        for cls in (Fail, jw.Unchecked, OSError):
            with self.subTest(cls=cls):
                self.out.seek(0)
                self.out.truncate()
                self.set("retire", exc=self.killed_exc(cls, "nope"))
                self.record(status="done")
                self.idle = [("s", "/p", "a")]
                self.runs.clear()
                out, err = self.reap()
                self.assertIn(f"stopped s in {self.path()}: AGENT_SESSION=s claude --resume sid\n", out)
                self.assertEqual(self.kills(), [])
                self.assertNotIn("stopped s in /p", out)
                jw.update_record(f"{self.state}/agent-sessions", "w", {}, drop=["reaped"])

    def test_mrs_failing_retire_reports_killed(self):
        for cls in (Fail, jw.Unchecked, OSError):
            with self.subTest(cls=cls):
                self.out.seek(0)
                self.out.truncate()
                self.set("retire", exc=self.killed_exc(cls, "nope"))
                self.record(mrs=["https://x/1"])
                self.idle = [("s", "/p", "a")]
                self.runs.clear()
                out, err = self.reap()
                self.assertIn(f"stopped s in {self.path()}: AGENT_SESSION=s claude --resume sid\n", out)
                self.assertEqual(self.kills(), [])
                self.assertNotIn("stopped s in /p", out)
                self.assertNotIn("reaped", self.read())

    def test_mrs_retire_kills_reported_and_excluded(self):
        self.set("retire", [("s1", "sid1")])
        self.record(mrs=["https://x/1"])
        self.idle = [("s1", "/p", "a"), ("s2", "/q", "b")]
        out, err = self.reap()
        self.assertIn(f"stopped s1 in {self.path()}: AGENT_SESSION=s1 claude --resume sid1\n", out)
        self.assertEqual(self.kills(), ["s2"])
        self.assertNotIn("stopped s1 in /p", out)

    # mrs path

    def test_mrs_live_merged_notifies(self):
        self.live_session()
        self.record(mrs=["https://x/1"])
        out, err = self.reap()
        self.assertIsNone(self.exc)
        self.m["deliver"].assert_called_once_with("w", jw.WRAP_UP, "jw-reap", wait=0)
        self.assertTrue(self.read()["notified"])
        self.assertNotIn("reaped", self.read())
        self.assertIn("notified w\n", out)
        for c in ("retire", "stop_sessions"):
            self.assertNotIn(c, self.calls)

    def test_mrs_live_merged_deliver_fail_still_notified(self):
        self.live_session()
        self.set("deliver", exc=Fail("no running session"))
        self.record(mrs=["https://x/1"])
        self.reap()
        self.assertTrue(self.read()["notified"])
        self.assertNotIn("retire", self.calls)

    def test_mrs_live_unmerged_nothing(self):
        self.live_session()
        self.set("stack_merged", False)
        self.record(mrs=["https://x/1"])
        self.reap()
        for c in ("deliver", "retire", "stop_sessions"):
            self.assertNotIn(c, self.calls)
        self.assertNotIn("notified", self.read())
        self.assertNotIn("reaped", self.read())

    def test_mrs_live_unknown_stderr_nothing(self):
        self.live_session()
        self.set("stack_merged", None)
        self.record(mrs=["https://x/1"])
        out, err = self.reap()
        self.assertNotEqual(err, "")
        for c in ("deliver", "retire", "stop_sessions"):
            self.assertNotIn(c, self.calls)
        self.assertNotIn("notified", self.read())

    def test_mrs_dead_retires_and_marks(self):
        self.record(mrs=["https://x/1"])
        out, err = self.reap()
        self.m["retire"].assert_called_once_with(self.home, "w", yes=True, idle=600)
        self.assertNotIn("deliver", self.calls)
        self.assertTrue(self.read()["reaped"])
        self.assertIn("removed r.agents/w", out)

    def test_mrs_notified_retires_even_if_live(self):
        self.live_session()
        self.record(mrs=["https://x/1"], notified=True)
        self.reap()
        self.m["retire"].assert_called_once_with(self.home, "w", yes=True, idle=600)
        self.assertNotIn("deliver", self.calls)
        self.assertTrue(self.read()["reaped"])

    def test_mrs_retire_unchecked_stderr_no_mark(self):
        self.set("retire", exc=jw.Unchecked("couldn't check the stack"))
        self.record(mrs=["https://x/1"])
        out, err = self.reap()
        self.assertNotEqual(err, "")
        self.assertNotIn("reaped", self.read())

    def test_mrs_retire_oserror_stderr_no_mark(self):
        self.set("retire", exc=OSError("disk"))
        self.record(mrs=["https://x/1"])
        out, err = self.reap()
        self.assertIn("disk", err)
        self.assertNotIn("reaped", self.read())
        self.assertNotIn("removed", out)

    def test_mrs_retire_fail_no_mark(self):
        self.set("retire", exc=Fail("unmerged work"))
        self.record(mrs=["https://x/1"])
        self.reap()
        self.assertNotIn("reaped", self.read())

    def test_mrs_gitlab_workspace_skipped(self):
        self.record("gl-ab-12", mrs=["https://x/1"])
        self.reap()
        for c in ("deliver", "retire", "stop_sessions"):
            self.assertNotIn(c, self.calls)
        self.assertNotIn("reaped", self.read("gl-ab-12"))
        self.assertNotIn("notified", self.read("gl-ab-12"))

    def test_several_records_independent(self):
        self.set("retire", exc=Fail("unmerged work"))
        self.record("a", status="done")
        self.record("b", status="done")
        self.reap()
        self.assertEqual(self.calls.count("retire"), 2)
        self.assertTrue(self.read("a")["reaped"])
        self.assertTrue(self.read("b")["reaped"])

    # lock

    def test_busy_lock_exits_zero_without_work(self):
        self.record(status="done")
        with open(f"{self.rt}/jw-reap.lock", "w") as f:
            fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)
            out, err = self.reap()
        if isinstance(self.exc, SystemExit):
            self.assertIn(self.exc.code, (0, None))
        else:
            self.assertIsNone(self.exc)
        self.assertIn("jw: reap already running", err)
        self.assertEqual(self.calls, [])
        self.assertNotIn("reaped", self.read())

    # robustness

    def test_bad_record_isolated(self):
        def retire(home, name, **k):
            self.calls.append("retire")
            if name == "a":
                raise ValueError("boom")
            return []

        self.m["retire"].side_effect = retire
        self.record("a", status="done")
        self.record("b", status="done")
        self.idle = [("s2", "/q", "b")]
        out, err = self.reap()
        self.assertIsNone(self.exc)
        self.assertIn("jw: a: ", err)
        self.assertIn("boom", err)
        self.assertEqual(self.calls.count("retire"), 2)
        self.assertTrue(self.read("b")["reaped"])
        self.assertEqual(self.kills(), ["s2"])

    def test_bad_record_mrs_path_isolated(self):
        self.set("stack_merged", exc=ValueError("bad"))
        self.live_session()
        self.record("a", mrs=["https://x/1"])
        self.record("b", status="done")
        self.idle = [("s2", "/q", "b")]
        out, err = self.reap()
        self.assertIsNone(self.exc)
        self.assertIn("jw: a: ", err)
        self.assertTrue(self.read("b")["reaped"])
        self.assertEqual(self.kills(), ["s2"])

    def test_deadline_skips_rest_and_runs_idle_pass(self):
        clock = [1000.0]
        orig = self.m["retire"].side_effect

        def retire(*a, **k):
            clock[0] += 700  # the first record takes over 10 min
            return orig(*a, **k)

        self.m["retire"].side_effect = retire
        self.record("a", status="done")
        self.record("b", status="done")
        self.idle = [("s2", "/q", "b")]
        with mock.patch.object(jw.time, "monotonic", lambda: clock[0]):
            out, err = self.reap()
        self.assertIsNone(self.exc)
        self.assertEqual(self.calls.count("retire"), 1)
        self.assertIn("out of time", err)
        self.assertTrue(self.read("a")["reaped"])
        self.assertNotIn("reaped", self.read("b"))
        self.assertEqual(self.kills(), ["s2"])

    def test_no_deadline_message_when_fast(self):
        self.record("a", status="done")
        out, err = self.reap()
        self.assertNotIn("out of time", err)

    # records that need no change are not rewritten

    def assert_untouched(self, name="w"):
        p = f"{self.state}/agent-sessions/{name}.json"
        os.utime(p, (1000, 1000))
        self.reap()
        self.assertEqual(os.stat(p).st_mtime, 1000)

    def test_skipped_record_not_rewritten(self):
        self.record(status="done", reaped=True)
        self.assert_untouched()

    def test_neither_done_nor_mrs_not_rewritten(self):
        self.record()
        self.assert_untouched()

    def test_unmerged_not_rewritten(self):
        self.live_session()
        self.set("stack_merged", False)
        self.record(mrs=["https://x/1"])
        self.assert_untouched()

    def test_mrs_unchecked_not_rewritten(self):
        self.set("retire", exc=jw.Unchecked("couldn't check the stack"))
        self.record(mrs=["https://x/1"])
        self.assert_untouched()

    def test_workspace_path_passed_to_helpers(self):
        self.live_session()
        self.record(mrs=["https://x/1"])
        self.reap()
        self.m["stack_merged"].assert_called_once_with(self.path())
        self.assertEqual(self.cs_args, [self.path()])

    def test_lock_released_after_run(self):
        self.reap()
        self.assertIsNone(self.exc)
        with open(f"{self.rt}/jw-reap.lock", "w") as f:
            fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)  # must not raise

    # tools

    def test_missing_tool_reported_once(self):
        self.missing = "glab"
        self.record("a", status="done")
        self.record("b", status="done")
        out, err = self.reap()
        lines = [l for l in err.splitlines() if l.startswith("jw: missing on PATH:")]
        self.assertEqual(len(lines), 1)
        self.assertIn("glab", lines[0])
        self.assertNotIn("jj", lines[0])

    def test_missing_git_reported(self):
        self.missing = "git"
        out, err = self.reap()
        lines = [l for l in err.splitlines() if l.startswith("jw: missing on PATH:")]
        self.assertEqual(len(lines), 1)
        self.assertIn("git", lines[0].split())

    def test_no_missing_tool_message_when_present(self):
        self.record(status="done")
        out, err = self.reap()
        self.assertNotIn("missing on PATH", err)

    # the 4 h pass

    def test_idle_pass_after_records_on_fresh_state(self):
        self.record(status="done")
        orig = self.m["stop_sessions"].side_effect

        def stop(*a, **k):
            self.live = ["fresh"]  # changes while the record loop runs
            return orig(*a, **k)

        self.m["stop_sessions"].side_effect = stop
        self.reap()
        self.assertEqual(self.idle_args[0], ["fresh"])
        last_live = len(self.calls) - 1 - self.calls[::-1].index("live_claudes")
        self.assertGreater(last_live, self.calls.index("retire"))
        self.assertGreater(self.calls.index("idle_sessions"), last_live)

    def kills(self):
        return [c[-1] for c in self.runs if "kill" in c]

    def test_idle_pass_excludes_stopped_names(self):
        self.set("stop_sessions", [("s1", "sid1")])
        self.record(status="done")
        self.idle = [("s1", "/p", "a"), ("s2", "/q", "b")]
        out, err = self.reap()
        self.assertEqual(self.kills(), ["s2"])
        self.assertIn("stopped s2 in /q: AGENT_SESSION=s2 claude --resume b", out)
        self.assertNotIn("stopped s1 in /p", out)

    def test_idle_pass_without_records_still_runs(self):
        self.idle = [("s2", "/q", "b")]
        out, err = self.reap()
        self.assertEqual(self.kills(), ["s2"])
        self.assertIn("stopped s2 in /q: AGENT_SESSION=s2 claude --resume b", out)


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
        # start time computed here: under qemu-user it differs per thread
        start = jw.stat_fields(os.getpid())[19]
        threading.Timer(0.6, self.register, (os.getpid(), "late", sock, None, start)).start()
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

    def test_drop_removes_after_merge(self):
        self.write("s.json", json.dumps({"kind": "task", "done": True, "notified": 1, "reaped": 2}))
        r = jw.update_record(self.d, "s", {"title": "T"}, drop=("done", "notified", "reaped", "absent"))
        self.assertEqual(r, {"kind": "task", "title": "T"})
        self.assertEqual(jw.read_record(self.d, "s"), {"kind": "task", "title": "T"})

    def test_drop_wins_over_fields(self):
        r = jw.update_record(self.d, "s", {"done": True}, drop=["done"])
        self.assertNotIn("done", r)
        self.assertNotIn("done", jw.read_record(self.d, "s"))

    def test_no_drop_keeps_all(self):
        self.write("s.json", json.dumps({"done": True}))
        self.assertEqual(jw.update_record(self.d, "s", {"kind": "x"}), {"done": True, "kind": "x"})

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

    FINISHED = {"status": "done", "status_at": 4, "cache_expires_at": 9, "notified": 5, "reaped": 6}

    def test_finish_marks(self):
        self.assertEqual(tuple(jw.FINISH_MARKS), ("status", "status_at", "waiting_for", "cache_expires_at", "notified", "reaped"))

    def test_set_workspace_drops_all_marks(self):
        self.write("s-1.json", json.dumps({"status": "done", "status_at": 1, "cache_expires_at": 9,
                                           "notified": 2, "reaped": 3, "title": "T"}))
        self.run_cmd("set", "--workspace", "/w")
        self.assertEqual(jw.read_record(self.d, "s-1"), {"title": "T", "workspace": "/w"})

    def test_set_workspace_drops_waiting_for(self):
        self.write("s-1.json", json.dumps({"status": "waiting", "waiting_for": "x", "status_at": 1}))
        self.run_cmd("set", "--workspace", "/w")
        self.assertEqual(jw.read_record(self.d, "s-1"), {"workspace": "/w"})

    def test_set_workspace_resets_finished(self):
        self.write("s-1.json", json.dumps({"kind": "task", "mrs": ["https://g/1"], **self.FINISHED}))
        self.run_cmd("set", "--workspace", "/w")
        self.assertEqual(jw.read_record(self.d, "s-1"),
                         {"kind": "task", "mrs": ["https://g/1"], "workspace": "/w"})

    def test_set_without_workspace_keeps_finished(self):
        self.write("s-1.json", json.dumps({"kind": "task", **self.FINISHED}))
        self.run_cmd("set", "--title", "T", "--mr", "https://g/1")
        self.assertEqual(jw.read_record(self.d, "s-1"),
                         {"kind": "task", "title": "T", "mrs": ["https://g/1"], **self.FINISHED})

    def test_set_workspace_creates_without_finished(self):
        self.run_cmd("set", "--workspace", "/w")
        self.assertEqual(jw.read_record(self.d, "s-1"), {"workspace": "/w"})

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
        # a nonexistent binary raises OSError natively but exits 127 under qemu-user, so mock run
        done = subprocess.CompletedProcess([], 0, stdout=b"1 2\n")
        with mock.patch.object(jw, "CKSUM", "/nonexistent/cksum"), \
                mock.patch.object(jw.subprocess, "run", return_value=done) as run:
            jw.project_key(p)
        self.assertEqual(run.call_args.args[0][0], "/nonexistent/cksum")

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
        a = mock.Mock(repo=repo, detach=False, last=last, session=None)
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


class CmdResumeSession(unittest.TestCase):
    """jw resume --session NAME: the transcript of the workspace in the record of NAME."""

    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.tmp = tmp.name
        self.root = os.path.join(self.tmp, "projects")
        self.home = os.path.join(self.root, "repo")
        os.makedirs(os.path.join(self.home, ".jj"))
        self.cfg = os.path.join(self.tmp, "claude")
        self.proj = os.path.join(self.cfg, "projects", jw.project_key(self.home))
        os.makedirs(self.proj)
        self.state = os.path.join(self.tmp, "state")
        self.d = os.path.join(self.state, "agent-sessions")
        os.makedirs(self.d)
        self.live = []
        self.shpool = {}
        self.w = self.workspace("w1")

    def workspace(self, name, create=True):
        p = os.path.join(self.root, "repo.agents", name)
        if create:
            os.makedirs(os.path.join(p, ".jj"))
        return p

    def record(self, name, workspace):
        with open(os.path.join(self.d, name + ".json"), "w") as f:
            json.dump({"kind": "task", "workspace": workspace}, f)

    def session(self, sid, mtime, text, cwd):
        p = os.path.join(self.proj, sid + ".jsonl")
        with open(p, "w") as f:
            f.write(json.dumps({"type": "user", "cwd": cwd, "message": {"role": "user", "content": text}}) + "\n")
        os.utime(p, (mtime, mtime))

    def resume(self, name="s-1", origin="", repo=None):
        self.err = io.StringIO()
        a = mock.Mock(repo=repo, detach=False, last=False, session=name)
        with contextlib.ExitStack() as st:
            st.enter_context(mock.patch.dict(os.environ, {"CLAUDE_CONFIG_DIR": self.cfg, "XDG_STATE_HOME": self.state,
                                                          "KNOWLEDGE_ORIGIN": origin}))
            st.enter_context(mock.patch.object(jw, "ROOT", self.root))
            st.enter_context(mock.patch.object(jw, "live_claudes", return_value=self.live))
            st.enter_context(mock.patch.object(jw, "shpool_status", side_effect=lambda: self.shpool))
            self.picker = st.enter_context(mock.patch.object(jw, "pick"))
            self.add_ws = st.enter_context(mock.patch.object(jw, "add_workspace", return_value="/new/ws"))
            self.fetch = st.enter_context(mock.patch.object(jw, "fetch"))
            self.launch = st.enter_context(mock.patch.object(jw, "launch"))
            st.enter_context(mock.patch("sys.stderr", self.err))
            jw.cmd_resume(a)

    def preferred(self):
        c = self.launch.call_args
        return c.kwargs["preferred"] if "preferred" in c.kwargs else c.args[5]

    def resumed(self):
        c = self.launch.call_args
        return c.kwargs["resume"] if "resume" in c.kwargs else c.args[6]

    def test_picks_transcript_of_the_workspace(self):
        w2 = self.workspace("w2")
        self.record("s-1", self.w)
        self.session("other-newest", 4000, "other", w2)
        self.session("mine-new", 3000, "mine new", self.w)
        self.session("mine-old", 1000, "mine old", self.w)
        self.resume()
        self.picker.assert_not_called()
        home, detach, argv, text = self.launch.call_args.args[:4]
        self.assertEqual(home, self.home)
        self.assertFalse(detach)
        self.assertEqual(argv, ["claude", "--resume", "mine-new"])
        self.assertEqual(text, "mine new")
        self.assertEqual(self.preferred(), "s-1")
        self.assertTrue(self.resumed())

    def test_live_newest_transcript_fails_without_falling_through(self):
        self.record("s-1", self.w)
        self.session("mine-new", 3000, "new", self.w)
        self.session("mine-old", 1000, "old", self.w)
        self.live = [(1, {"sessionId": "mine-new"}, self.w, None)]
        with self.assertRaisesRegex(Fail, "session s-1 is running"):
            self.resume()
        self.launch.assert_not_called()

    def test_live_shpool_session_fails(self):
        self.record("s-1", self.w)
        self.session("a-1", 1000, "task", self.w)
        for state in ("attached", "disconnected"):
            with self.subTest(state=state):
                self.shpool = {"s-1": state}
                with self.assertRaisesRegex(Fail, "session s-1 is running"):
                    self.resume()
                self.launch.assert_not_called()

    def test_missing_workspace_recreated(self):
        gone = self.workspace("gone", create=False)
        self.record("s-1", gone)
        self.session("a-1", 1000, "task", gone)
        self.resume()
        self.assertEqual(self.launch.call_args.args[2], ["claude", "--resume", "a-1"])
        self.assertEqual(self.preferred(), "s-1")
        with mock.patch.object(jw, "add_workspace", return_value="/new/ws") as add_ws, \
                mock.patch.object(jw, "fetch"), mock.patch("sys.stderr", self.err):
            self.assertEqual(self.launch.call_args.args[4]("s-1"), "/new/ws")
        self.assertEqual(add_ws.call_args.args[:3], (self.home, "trunk()", "s-1"))

    def test_gone_workspace_outside_root_fails(self):
        gone = os.path.join(self.tmp, "elsewhere", "x")
        self.record("s-1", gone)
        self.session("a-1", 1000, "task", gone)
        with self.assertRaisesRegex(Fail, "no repo for session s-1"):
            self.resume()
        self.launch.assert_not_called()

    def test_repo_argument_fails_before_reading_record(self):
        with mock.patch.object(jw, "read_record") as rr, \
                self.assertRaisesRegex(Fail, "--session takes no repo"):
            self.resume(repo="repo")
        rr.assert_not_called()
        self.launch.assert_not_called()

    def test_existing_workspace_not_recreated(self):
        self.record("s-1", self.w)
        self.session("a-1", 1000, "task", self.w)
        self.resume()
        with mock.patch.object(jw, "add_workspace") as add_ws:
            self.assertEqual(self.launch.call_args.args[4]("s-1"), self.w)
        add_ws.assert_not_called()

    def test_no_record_fails(self):
        self.session("a-1", 1000, "task", self.w)
        with self.assertRaisesRegex(Fail, "no session record s-1"):
            self.resume()
        self.launch.assert_not_called()

    def test_no_matching_transcript_fails(self):
        self.record("s-1", self.w)
        self.session("other", 1000, "other", self.workspace("w2"))
        with self.assertRaisesRegex(Fail, "no sessions to resume for s-1"):
            self.resume()
        self.launch.assert_not_called()

    def test_invalid_name_fails(self):
        with self.assertRaisesRegex(Fail, "invalid session name"):
            self.resume("../x")
        self.launch.assert_not_called()

    def test_event_session_fails(self):
        self.record("s-1", self.w)
        self.session("a-1", 1000, "task", self.w)
        with self.assertRaises(Fail):
            self.resume(origin="event:comment")
        self.launch.assert_not_called()

    def parse(self, *args):
        err = io.StringIO()
        with mock.patch.object(sys, "argv", ["jw", "resume", *args]), contextlib.redirect_stderr(err):
            return jw.parse_args()

    def test_cli_session_option(self):
        a = self.parse("-d", "--session", "s-1")
        self.assertIs(a.func, jw.cmd_resume)
        self.assertEqual(a.session, "s-1")
        self.assertTrue(a.detach)

    def test_usage_error_with_last(self):
        for flag in ("-l", "--last"):
            with self.subTest(flag=flag):
                with self.assertRaises(SystemExit) as cm:
                    self.parse("--session", "s-1", flag)
                self.assertEqual(cm.exception.code, 2)

    def test_last_with_repo_still_parses(self):
        a = self.parse("-l", "repo")
        self.assertTrue(a.last)
        self.assertEqual(a.repo, "repo")


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
        self.assertIn("exec direnv exec . claude --resume sid\n", self.job)
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

    def test_resume_drops_finish_marks(self):
        self.seed()
        with self.state():
            jw.update_record(jw.records_dir(), "w1", {"status": "done", "status_at": 1, "waiting_for": "q",
                                                      "cache_expires_at": 9, "notified": 2, "reaped": 3})
        self.go([], "w1", resume=True)
        r = self.record()
        for k in ("status", "status_at", "waiting_for", "cache_expires_at", "notified", "reaped"):
            self.assertNotIn(k, r)
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


class CmdDone(RecordsBase):
    def setUp(self):
        super().setUp()
        self.d = os.path.join(self.tmp, "state", "agent-sessions")
        env = mock.patch.dict(os.environ, {"XDG_STATE_HOME": os.path.join(self.tmp, "state"),
                                           "AGENT_SESSION": "s-1", "KNOWLEDGE_ORIGIN": ""})
        env.start()
        self.addCleanup(env.stop)

    def run_cmd(self):
        with mock.patch.object(sys, "argv", ["jw", "done"]):
            a = jw.parse_args()
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            a.func(a)
        return err.getvalue()

    def done_record(self, **extra):
        r = jw.read_record(self.d, "s-1")
        self.assertIsInstance(r["status_at"], (int, float))
        self.assertNotIsInstance(r["status_at"], bool)
        self.assertNotIn("done", r)
        return {**r, "status_at": "T"} | extra

    def test_writes_done(self):
        err = self.run_cmd()
        self.assertEqual(self.done_record(), {"status": "done", "status_at": "T"})
        self.assertIn("jw: s-1 is done; it is stopped once it is idle and detached", err)

    def test_status_at_uses_time(self):
        with mock.patch.object(jw.time, "time", return_value=1234.5):
            self.run_cmd()
        self.assertEqual(jw.read_record(self.d, "s-1"), {"status": "done", "status_at": 1234.5})

    def test_keeps_other_keys(self):
        self.write("s-1.json", json.dumps({"title": "T", "mrs": ["https://g/1"]}))
        self.run_cmd()
        self.assertEqual(self.done_record(), {"title": "T", "mrs": ["https://g/1"], "status": "done",
                                              "status_at": "T"})

    def test_replaces_waiting(self):
        self.write("s-1.json", json.dumps({"status": "waiting", "waiting_for": "question", "title": "T",
                                           "mrs": ["https://g/1"]}))
        self.run_cmd()
        self.assertEqual(self.done_record(), {"status": "done", "status_at": "T", "title": "T",
                                              "mrs": ["https://g/1"]})

    def test_rerun_rewrites_status_at(self):
        with mock.patch.object(jw.time, "time", return_value=10):
            self.run_cmd()
        with mock.patch.object(jw.time, "time", return_value=20):
            self.run_cmd()
        self.assertEqual(jw.read_record(self.d, "s-1"), {"status": "done", "status_at": 20})

    def test_creates_missing_record_and_dir(self):
        self.assertFalse(os.path.exists(self.d))
        self.run_cmd()
        with open(self.path("s-1.json")) as f:
            r = json.load(f)
        self.assertEqual(r["status"], "done")
        self.assertEqual(set(r), {"status", "status_at"})

    def test_no_session(self):
        with mock.patch.dict(os.environ):
            del os.environ["AGENT_SESSION"]
            with self.assertRaises(Fail) as c:
                self.run_cmd()
        self.assertEqual(str(c.exception), "not in an agent session")
        self.assertFalse(os.path.exists(self.d))

    def test_empty_session(self):
        with mock.patch.dict(os.environ, {"AGENT_SESSION": ""}):
            with self.assertRaises(Fail) as c:
                self.run_cmd()
        self.assertEqual(str(c.exception), "not in an agent session")

    def test_invalid_name(self):
        for name in ["../x", "a b", "-x"]:
            with self.subTest(name=name), mock.patch.dict(os.environ, {"AGENT_SESSION": name}):
                with self.assertRaises(Fail) as c:
                    self.run_cmd()
                self.assertEqual(str(c.exception), f"invalid session name: {name}")
                self.assertFalse(os.path.exists(self.d))

    def test_event_origin_allowed(self):
        with mock.patch.dict(os.environ, {"KNOWLEDGE_ORIGIN": "event:x"}):
            self.run_cmd()
        self.assertEqual(jw.read_record(self.d, "s-1")["status"], "done")


class SetStatus(RecordsBase):
    def mtime(self, name="s.json"):
        return os.stat(self.path(name)).st_mtime_ns

    def test_creates_dir_and_record(self):
        r = jw.set_status(self.d, "s", "busy", now=5)
        self.assertEqual(r, {"status": "busy", "status_at": 5})
        self.assertEqual(jw.read_record(self.d, "s"), {"status": "busy", "status_at": 5})

    def test_now_defaults_to_time(self):
        with mock.patch.object(jw.time, "time", return_value=77.5):
            r = jw.set_status(self.d, "s", "idle")
        self.assertEqual(r, {"status": "idle", "status_at": 77.5})

    def test_same_status_no_write(self):
        self.write("s.json", json.dumps({"status": "busy", "status_at": 1}))
        self.age("s.json", 1)
        m = self.mtime()
        r = jw.set_status(self.d, "s", "busy", now=9)
        self.assertEqual(r, {"status": "busy", "status_at": 1})
        self.assertEqual(jw.read_record(self.d, "s"), {"status": "busy", "status_at": 1})
        self.assertEqual(self.mtime(), m)

    def test_same_waiting_for_no_write(self):
        self.write("s.json", json.dumps({"status": "waiting", "waiting_for": "a", "status_at": 1}))
        self.age("s.json", 1)
        m = self.mtime()
        jw.set_status(self.d, "s", "waiting", "a", now=9)
        self.assertEqual(jw.read_record(self.d, "s")["status_at"], 1)
        self.assertEqual(self.mtime(), m)

    def test_busy_to_waiting(self):
        jw.set_status(self.d, "s", "busy", now=1)
        r = jw.set_status(self.d, "s", "waiting", "permission: Bash", now=2)
        self.assertEqual(r, {"status": "waiting", "waiting_for": "permission: Bash", "status_at": 2})
        self.assertEqual(jw.read_record(self.d, "s"), r)

    def test_waiting_to_busy_removes_waiting_for(self):
        jw.set_status(self.d, "s", "waiting", "q", now=1)
        r = jw.set_status(self.d, "s", "busy", now=2)
        self.assertEqual(r, {"status": "busy", "status_at": 2})
        self.assertEqual(jw.read_record(self.d, "s"), r)

    def test_waiting_text_change_rewrites(self):
        jw.set_status(self.d, "s", "waiting", "a", now=1)
        r = jw.set_status(self.d, "s", "waiting", "b", now=2)
        self.assertEqual(r, {"status": "waiting", "waiting_for": "b", "status_at": 2})
        self.assertEqual(jw.read_record(self.d, "s"), r)

    def test_waiting_without_text_after_text_rewrites(self):
        jw.set_status(self.d, "s", "waiting", "a", now=1)
        r = jw.set_status(self.d, "s", "waiting", now=2)
        self.assertEqual(r, {"status": "waiting", "status_at": 2})

    def test_keeps_other_keys(self):
        self.write("s.json", json.dumps({"title": "T", "mrs": ["https://g/1"]}))
        r = jw.set_status(self.d, "s", "idle", now=3)
        self.assertEqual(r, {"title": "T", "mrs": ["https://g/1"], "status": "idle", "status_at": 3})
        self.assertEqual(jw.read_record(self.d, "s"), r)

    def test_done_never_overwritten(self):
        rec = {"status": "done", "status_at": 1, "title": "T"}
        for state, wf in (("busy", None), ("idle", None), ("waiting", "x"), ("ended", None)):
            with self.subTest(state=state):
                self.write("s.json", json.dumps(rec))
                self.age("s.json", 1)
                m = self.mtime()
                jw.set_status(self.d, "s", state, wf, now=9)
                self.assertEqual(jw.read_record(self.d, "s"), rec)
                self.assertEqual(self.mtime(), m)

    def seed(self, rec, aged=False):
        self.write("s.json", json.dumps(rec))
        if aged:
            self.age("s.json", 1)

    def test_cache_set_on_new_record(self):
        r = jw.set_status(self.d, "s", "idle", now=5, cache_expires_at=100)
        want = {"status": "idle", "status_at": 5, "cache_expires_at": 100}
        self.assertEqual(r, want)
        self.assertEqual(jw.read_record(self.d, "s"), want)

    def test_cache_update_same_status(self):
        self.seed({"status": "idle", "status_at": 1, "cache_expires_at": 50}, aged=True)
        m = self.mtime()
        r = jw.set_status(self.d, "s", "idle", now=9, cache_expires_at=100)
        want = {"status": "idle", "status_at": 1, "cache_expires_at": 100}
        self.assertEqual(r, want)
        self.assertEqual(jw.read_record(self.d, "s"), want)
        self.assertNotEqual(self.mtime(), m)

    def test_cache_unchanged_no_write(self):
        rec = {"status": "idle", "status_at": 1, "cache_expires_at": 50}
        self.seed(rec, aged=True)
        m = self.mtime()
        jw.set_status(self.d, "s", "idle", now=9, cache_expires_at=50)
        self.assertEqual(jw.read_record(self.d, "s"), rec)
        self.assertEqual(self.mtime(), m)

    def test_cache_removed_on_status_change(self):
        self.seed({"status": "idle", "status_at": 1, "cache_expires_at": 50})
        r = jw.set_status(self.d, "s", "busy", now=2)
        self.assertEqual(r, {"status": "busy", "status_at": 2})
        self.assertEqual(jw.read_record(self.d, "s"), r)

    def test_cache_removed_same_status(self):
        self.seed({"status": "busy", "status_at": 1, "cache_expires_at": 50}, aged=True)
        m = self.mtime()
        r = jw.set_status(self.d, "s", "busy")
        self.assertEqual(r, {"status": "busy", "status_at": 1})
        self.assertEqual(jw.read_record(self.d, "s"), r)
        self.assertNotEqual(self.mtime(), m)

    def test_cache_set_on_done(self):
        self.seed({"status": "done", "status_at": 1})
        jw.set_status(self.d, "s", "idle", now=9, cache_expires_at=100)
        self.assertEqual(jw.read_record(self.d, "s"),
                         {"status": "done", "status_at": 1, "cache_expires_at": 100})

    def test_cache_removed_on_done(self):
        self.seed({"status": "done", "status_at": 1, "cache_expires_at": 100})
        jw.set_status(self.d, "s", "busy", now=9)
        self.assertEqual(jw.read_record(self.d, "s"), {"status": "done", "status_at": 1})

    def test_cache_unchanged_on_done_no_write(self):
        rec = {"status": "done", "status_at": 1, "cache_expires_at": 100}
        self.seed(rec, aged=True)
        m = self.mtime()
        jw.set_status(self.d, "s", "idle", now=9, cache_expires_at=100)
        self.assertEqual(jw.read_record(self.d, "s"), rec)
        self.assertEqual(self.mtime(), m)


class CmdSessionStatus(RecordsBase):
    def setUp(self):
        super().setUp()
        self.d = os.path.join(self.tmp, "state", "agent-sessions")
        env = mock.patch.dict(os.environ, {"XDG_STATE_HOME": os.path.join(self.tmp, "state"),
                                           "AGENT_SESSION": "s-1", "KNOWLEDGE_ORIGIN": ""})
        env.start()
        self.addCleanup(env.stop)

    def run_cmd(self, *args):
        with mock.patch.object(sys, "argv", ["jw", "session", "status", *args]):
            a = jw.parse_args()
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            a.func(a)
        return out.getvalue()

    def rec(self):
        return jw.read_record(self.d, "s-1")

    def test_states(self):
        for state in ("busy", "idle", "ended"):
            with self.subTest(state=state):
                self.assertEqual(self.run_cmd(state), "")
                r = self.rec()
                self.assertEqual(r["status"], state)
                self.assertIsInstance(r["status_at"], (int, float))
                self.assertNotIn("waiting_for", r)

    def test_waiting_with_text(self):
        self.assertEqual(self.run_cmd("waiting", "--waiting-for", "permission: Bash"), "")
        r = self.rec()
        self.assertEqual((r["status"], r["waiting_for"]), ("waiting", "permission: Bash"))

    def test_waiting_without_text(self):
        self.run_cmd("waiting")
        self.assertNotIn("waiting_for", self.rec())

    def test_waiting_then_busy_clears(self):
        self.run_cmd("waiting", "--waiting-for", "x")
        self.run_cmd("busy")
        self.assertNotIn("waiting_for", self.rec())

    def test_invalid_state_exits_2(self):
        for state in ("done", "bogus"):
            with self.subTest(state=state), contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit) as c:
                    self.run_cmd(state)
                self.assertEqual(c.exception.code, 2)
                self.assertFalse(os.path.exists(self.d))

    def test_no_session(self):
        with mock.patch.dict(os.environ):
            del os.environ["AGENT_SESSION"]
            with self.assertRaises(Fail) as c:
                self.run_cmd("busy")
        self.assertEqual(str(c.exception), "not in an agent session")
        self.assertFalse(os.path.exists(self.d))

    def test_empty_session(self):
        with mock.patch.dict(os.environ, {"AGENT_SESSION": ""}):
            with self.assertRaises(Fail) as c:
                self.run_cmd("busy")
        self.assertEqual(str(c.exception), "not in an agent session")

    def test_invalid_name(self):
        for name in ["../x", "a b", "-x"]:
            with self.subTest(name=name), mock.patch.dict(os.environ, {"AGENT_SESSION": name}):
                with self.assertRaises(Fail) as c:
                    self.run_cmd("busy")
                self.assertEqual(str(c.exception), f"invalid session name: {name}")
                self.assertFalse(os.path.exists(self.d))

    def test_event_origin_allowed(self):
        with mock.patch.dict(os.environ, {"KNOWLEDGE_ORIGIN": "event:x"}):
            self.run_cmd("busy")
        self.assertEqual(self.rec()["status"], "busy")

    def test_waiting_for_needs_waiting(self):
        for state in ("busy", "idle", "ended"):
            with self.subTest(state=state):
                with self.assertRaises(Fail) as c:
                    self.run_cmd(state, "--waiting-for", "x")
                self.assertEqual(str(c.exception), "--waiting-for needs waiting")
                self.assertFalse(os.path.exists(self.d))

    def test_text_sanitised(self):
        self.run_cmd("waiting", "--waiting-for", "x\ny\x1b")
        self.assertEqual(self.rec()["waiting_for"], "x y")

    def test_text_truncated(self):
        self.run_cmd("waiting", "--waiting-for", "a" * 300)
        self.assertEqual(self.rec()["waiting_for"], "a" * 200)

    def test_empty_text_no_key(self):
        for text in ("", "  \n "):
            with self.subTest(text=text):
                self.run_cmd("waiting", "--waiting-for", text)
                self.assertEqual(self.rec()["status"], "waiting")
                self.assertNotIn("waiting_for", self.rec())

    def test_done_record_untouched(self):
        self.write("s-1.json", json.dumps({"status": "done", "status_at": 1}))
        self.run_cmd("busy")
        self.assertEqual(self.rec(), {"status": "done", "status_at": 1})

    def test_cache_expires_at_int(self):
        self.assertEqual(self.run_cmd("idle", "--cache-expires-at", "1234"), "")
        r = self.rec()
        self.assertEqual(r["status"], "idle")
        self.assertEqual(r["cache_expires_at"], 1234)
        self.assertIsInstance(r["cache_expires_at"], int)

    def test_cache_expires_at_removed_without_flag(self):
        self.run_cmd("idle", "--cache-expires-at", "1234")
        self.run_cmd("idle")
        self.assertNotIn("cache_expires_at", self.rec())

    def test_cache_expires_at_with_waiting_for(self):
        self.run_cmd("waiting", "--waiting-for", "x", "--cache-expires-at", "7")
        r = self.rec()
        self.assertEqual((r["waiting_for"], r["cache_expires_at"]), ("x", 7))

    def test_cache_expires_at_not_integer_exits_2(self):
        for v in ("abc", "1.5"):
            with self.subTest(v=v), contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaises(SystemExit) as c:
                    self.run_cmd("idle", "--cache-expires-at", v)
                self.assertEqual(c.exception.code, 2)
                self.assertFalse(os.path.exists(self.d))


class Deliver(unittest.TestCase):
    def test_wait_zero_single_attempt(self):
        with mock.patch.object(jw, "message_sessions", return_value=[]) as m, \
                mock.patch.object(time, "sleep") as sl:
            with self.assertRaises(Fail) as c:
                jw.deliver("ghost", "hi", "me", wait=0)
        self.assertEqual(str(c.exception), "no running session ghost")
        self.assertEqual(m.call_count, 1)
        sl.assert_not_called()


def cp(rc, out=""):
    return subprocess.CompletedProcess([], rc, stdout=out)


class StackMerged(unittest.TestCase):
    def run_with(self, **kw):
        with mock.patch.object(subprocess, "run", **kw) as m:
            return jw.stack_merged("/d"), m

    def test_call(self):
        _, m = self.run_with(return_value=cp(0, "merged\n"))
        args, kwargs = m.call_args
        self.assertEqual(args[0], ["jj", "stack", "status"])
        self.assertEqual(kwargs["cwd"], "/d")
        self.assertEqual(kwargs["stdin"], subprocess.DEVNULL)
        self.assertEqual(kwargs["stdout"], subprocess.PIPE)
        self.assertNotIn("stderr", kwargs)
        self.assertEqual(kwargs["timeout"], 300)

    def test_merged(self):
        self.assertIs(self.run_with(return_value=cp(0, "x\nmerged\n\n"))[0], True)

    def test_merged_not_last_line(self):
        self.assertIsNone(self.run_with(return_value=cp(0, "merged\nother\n"))[0])

    def test_plain_log_output(self):
        self.assertIsNone(self.run_with(return_value=cp(0, "@  abc 1 file\n"))[0])

    def test_rc1(self):
        self.assertIs(self.run_with(return_value=cp(1, "unmerged\n"))[0], False)

    def test_rc2(self):
        self.assertIsNone(self.run_with(return_value=cp(2, "merged\n"))[0])

    def test_other_rc(self):
        self.assertIsNone(self.run_with(return_value=cp(127))[0])

    def test_oserror(self):
        self.assertIsNone(self.run_with(side_effect=OSError("no jj"))[0])

    def test_timeout(self):
        self.assertIsNone(self.run_with(side_effect=subprocess.TimeoutExpired("jj", 300))[0])

    def test_unicode_error(self):
        err = UnicodeDecodeError("utf-8", b"\xff", 0, 1, "invalid")
        self.assertIsNone(self.run_with(side_effect=err)[0])

    def test_unchecked_is_fail(self):
        self.assertTrue(issubclass(jw.Unchecked, Fail))


class Confirm(unittest.TestCase):
    def test_yes_skips_tty(self):
        err = io.StringIO()
        with mock.patch("builtins.open", side_effect=AssertionError("opened")) as op, \
                mock.patch.object(sys, "stderr", err):
            jw.confirm("Delete? [y/N] ", yes=True)
        op.assert_not_called()
        self.assertIn("Delete? [y/N] ", err.getvalue())
        self.assertIn("y (--yes)", err.getvalue())

    def test_ask_lost_passes_yes(self):
        with mock.patch.object(jw, "jj", return_value=""), \
                mock.patch.object(jw, "untracked", return_value=["a", "b"]), \
                mock.patch.object(jw, "confirm") as c:
            jw.ask_lost("/d", yes=True)
        c.assert_called_once()
        a, k = c.call_args
        self.assertTrue(k.get("yes", a[1] if len(a) > 1 else False))


class AskLostReal(unittest.TestCase):
    """ask_lost with the real confirm and a real directory; only jj is mocked."""

    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.d = tmp.name
        os.makedirs(f"{self.d}/.jj")

    def ask(self):
        err = io.StringIO()
        with mock.patch.object(jw, "jj", return_value=""), mock.patch.object(sys, "stderr", err):
            jw.ask_lost(self.d, yes=True)
        return err.getvalue()

    def touch(self, rel):
        os.makedirs(os.path.dirname(f"{self.d}/{rel}"), exist_ok=True)
        open(f"{self.d}/{rel}", "w").close()

    def test_yes_lists_every_file(self):
        names = [f"file{i}.txt" for i in range(12)]
        for n in names:
            self.touch(n)
        out = self.ask()
        for n in names:
            self.assertIn(n, out)
        self.assertNotIn("more", out)
        self.assertIn("y (--yes)", out)

    def test_yes_collapses_big_directory(self):
        for i in range(30):
            self.touch(f"big/f{i}")
        for i in range(8):
            self.touch(f"top{i}")
        out = self.ask()
        for i in range(8):
            self.assertIn(f"top{i}", out)
        self.assertIn("big", out)
        self.assertIn("30", out)
        self.assertNotIn("big/f7", out)
        self.assertNotIn("big/f29", out)


class CheckClaudes(unittest.TestCase):
    NOW = 1_000_000.0

    def check(self, sessions, idle=3600, state=None, top=True):
        state = {"s": "disconnected"} if state is None else state
        with mock.patch.object(jw, "descendants", return_value={1}), \
                mock.patch.object(jw, "in_use", return_value=False), \
                mock.patch.object(jw, "session_top", return_value=top) as st, \
                mock.patch.object(jw, "shpool_status", return_value=state) as ss, \
                mock.patch.object(time, "time", return_value=self.NOW):
            r = jw.check_claudes("/d", sessions, idle)
        return r, st, ss

    def sess(self, status="idle", age=7200, name="s"):
        return {10: (status, name, (self.NOW - age) * 1000, "sid")}

    def test_idle_passes(self):
        r, st, ss = self.check(self.sess())
        self.assertEqual(r, ["s"])
        st.assert_called()
        ss.assert_called()

    def test_waiting_fails(self):
        with self.assertRaises(Fail):
            self.check(self.sess(status="waiting"))

    def test_attached_fails(self):
        with self.assertRaises(Fail):
            self.check(self.sess(), state={"s": "attached"})

    def test_event_launcher_parent_passes(self):
        # real session_top over a fake /proc: Claude -> event-claude-la -> shpool
        proc = tempfile.TemporaryDirectory()
        self.addCleanup(proc.cleanup)
        for pid, ppid, comm in [(10, 20, "claude"), (20, 1, "event-claude-la"), (1, 0, "shpool")]:
            os.makedirs(f"{proc.name}/{pid}")
            with open(f"{proc.name}/{pid}/stat", "w") as f:
                f.write(f"{pid} (x) S {ppid} 1 1")
            with open(f"{proc.name}/{pid}/comm", "w") as f:
                f.write(comm + "\n")
        real = jw.session_top
        with mock.patch.object(jw, "session_top", side_effect=lambda pid: real(pid, proc.name)) as st, \
                mock.patch.object(jw, "descendants", return_value={10}), \
                mock.patch.object(jw, "in_use", return_value=False), \
                mock.patch.object(jw, "shpool_status", return_value={"s": "disconnected"}), \
                mock.patch.object(time, "time", return_value=self.NOW):
            self.assertEqual(jw.check_claudes("/d", self.sess(), 3600), ["s"])
        st.assert_called()

    def test_not_session_top_fails(self):
        with self.assertRaises(Fail):
            self.check(self.sess(), top=False)

    def test_idle_threshold(self):
        self.assertEqual(self.check(self.sess(age=600), idle=300)[0], ["s"])
        with self.assertRaises(Fail):
            self.check(self.sess(age=600), idle=1200)
        with self.assertRaises(Fail):
            self.check(self.sess(age=7200), idle=14400)

    def test_no_shpool_session_fails(self):
        with self.assertRaises(Fail):
            self.check(self.sess(name=None))


class ClaudeSessions(unittest.TestCase):
    def test_four_tuples(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        d = os.path.realpath(tmp.name)
        os.makedirs(f"{d}/sub")
        live = [(10, {"status": "idle", "statusUpdatedAt": 5, "sessionId": "sid1"}, d, "s1"),
                (11, {"status": "busy", "statusUpdatedAt": 6, "sessionId": "sid2"}, f"{d}/sub", None),
                (12, {"status": "idle", "statusUpdatedAt": 7, "sessionId": "sid3"}, "/elsewhere", "s3")]
        with mock.patch.object(jw, "live_claudes", return_value=live):
            r = jw.claude_sessions(d)
        self.assertEqual(r, {10: ("idle", "s1", 5, "sid1"), 11: ("busy", None, 6, "sid2")})


class StopSessions(unittest.TestCase):
    def test_none_not_in_use(self):
        with mock.patch.object(jw, "claude_sessions", return_value={}), \
                mock.patch.object(jw, "in_use", return_value=False), \
                mock.patch.object(jw, "kill_shpool") as k:
            self.assertEqual(jw.stop_sessions("/d", 3600), [])
        k.assert_not_called()

    def test_none_in_use_fails(self):
        with mock.patch.object(jw, "claude_sessions", return_value={}), \
                mock.patch.object(jw, "in_use", return_value=True):
            with self.assertRaises(Fail):
                jw.stop_sessions("/d", 3600)

    def test_kills_and_returns(self):
        sessions = {10: ("idle", "s1", 1, "sid1"), 11: ("idle", "s2", 1, "sid2")}
        with mock.patch.object(jw, "claude_sessions", return_value=sessions), \
                mock.patch.object(jw, "check_claudes", return_value=["s1", "s2"]) as c, \
                mock.patch.object(jw, "kill_shpool") as k:
            r = jw.stop_sessions("/d", 99)
        self.assertEqual(sorted(r), [("s1", "sid1"), ("s2", "sid2")])
        self.assertEqual(c.call_args[0][:2], ("/d", sessions))
        self.assertIn(99, list(c.call_args[0][2:]) + list(c.call_args[1].values()))
        k.assert_called()

    def test_check_failure_kills_nothing(self):
        sessions = {10: ("waiting", "s1", 1, "sid1")}
        with mock.patch.object(jw, "claude_sessions", return_value=sessions), \
                mock.patch.object(jw, "check_claudes", side_effect=Fail("busy")), \
                mock.patch.object(jw, "kill_shpool") as k:
            with self.assertRaises(Fail):
                jw.stop_sessions("/d", 3600)
        k.assert_not_called()


class Retire(unittest.TestCase):
    D = "/h.agents/w"
    HEADS = "heads(trunk().. ~ ::(working_copies() | remote_bookmarks()))"

    def setUp(self):
        self.calls = []
        self.at = "aaa\n"
        self.merged = True
        self.sessions = {10: ("idle", "s", 1, "sid")}

        def merged(d):
            self.calls.append("stack_merged")
            return self.merged

        def kill(*a, **k):
            self.calls.append("stop_sessions")
            return [("s", "sid")]

        def ask(*a, **k):
            self.calls.append("ask_lost")

        def rm(*a, **k):
            self.calls.append("remove")

        self.heads = ""
        self.heads_fail = False
        self.jj_calls = []

        def jjfn(*a, **k):
            self.jj_calls.append((a, k))
            if self.HEADS in a:
                self.calls.append("heads")
                if self.heads_fail:
                    raise Fail("jj failed")
                return self.heads
            return self.at

        self.m = {}
        for n, kw in [("in_use", {"return_value": False}),
                      ("claude_sessions", {"side_effect": lambda d: self.sessions}),
                      ("check_claudes", {"return_value": ["s"]}),
                      ("stack_merged", {"side_effect": merged}),
                      ("stop_sessions", {"side_effect": kill}),
                      ("ask_lost", {"side_effect": ask}),
                      ("jj", {"side_effect": jjfn}),
                      ("remove", {"side_effect": rm})]:
            p = mock.patch.object(jw, n, **kw)
            self.m[n] = p.start()
            self.addCleanup(p.stop)
        p = mock.patch.object(sys, "stderr", io.StringIO())
        p.start()
        self.addCleanup(p.stop)

    def retire(self, yes=False):
        return jw.retire("/h", "w", yes=yes, idle=3600)

    def test_happy(self):
        r = self.retire()
        self.assertEqual(r, [("s", "sid")])
        self.assertEqual([c for c in self.calls if c in ("stack_merged", "stop_sessions", "ask_lost", "remove")],
                         ["stack_merged", "ask_lost", "stop_sessions", "ask_lost", "remove"])
        self.m["remove"].assert_called_once_with("/h", "w")
        self.m["stop_sessions"].assert_called_once_with(self.D, 3600)

    def test_ask_lost_gets_yes(self):
        self.retire(yes=True)
        a, k = self.m["ask_lost"].call_args
        self.assertEqual(a[0], self.D)
        self.assertTrue(k.get("yes", a[1] if len(a) > 1 else False))

    def test_unmerged(self):
        self.merged = False
        with self.assertRaises(Fail) as c:
            self.retire()
        self.assertEqual(str(c.exception), "unmerged work")
        self.assertNotIsInstance(c.exception, jw.Unchecked)
        self.m["stop_sessions"].assert_not_called()
        self.m["remove"].assert_not_called()

    def test_unchecked_before_kill(self):
        self.merged = None
        with self.assertRaises(jw.Unchecked):
            self.retire()
        self.m["stop_sessions"].assert_not_called()
        self.m["ask_lost"].assert_not_called()
        self.m["remove"].assert_not_called()

    def test_in_use_needs_sessions(self):
        self.m["in_use"].return_value = True
        self.sessions = {}
        with self.assertRaises(Fail):
            self.retire()
        self.m["stop_sessions"].assert_not_called()
        self.m["remove"].assert_not_called()

    def test_in_use_check_claudes_fails(self):
        self.m["in_use"].return_value = True
        self.m["check_claudes"].side_effect = Fail("busy")
        with self.assertRaises(Fail):
            self.retire()
        self.m["stack_merged"].assert_not_called()
        self.m["stop_sessions"].assert_not_called()

    def test_in_use_passing_continues(self):
        self.m["in_use"].return_value = True
        self.assertEqual(self.retire(), [("s", "sid")])
        self.m["remove"].assert_called_once()

    def test_changed_at_after_ask_keeps_workspace(self):
        def ask(*a, **k):
            self.at = "bbb\n"
        self.m["ask_lost"].side_effect = ask
        with self.assertRaises(Fail):
            self.retire()
        self.m["remove"].assert_not_called()

    def test_changed_at_after_kill_keeps_workspace(self):
        def kill(*a, **k):
            self.at = "ccc\n"
            return [("s", "sid")]
        self.m["stop_sessions"].side_effect = kill
        with self.assertRaises(Fail):
            self.retire()
        self.m["remove"].assert_not_called()

    def order(self):
        return [c for c in self.calls if c in ("stack_merged", "stop_sessions", "ask_lost", "heads", "remove")]

    def test_yes_order(self):
        self.retire(yes=True)
        self.assertEqual(self.order(), ["stack_merged", "stop_sessions", "ask_lost", "heads", "remove"])

    def test_no_yes_asks_before_kill(self):
        self.retire(yes=False)
        o = self.order()
        self.assertLess(o.index("ask_lost"), o.index("stop_sessions"))
        self.assertLess(o.index("stop_sessions"), o.index("remove"))

    def test_no_yes_answer_no_kills_nothing(self):
        self.m["ask_lost"].side_effect = Fail("nothing removed")
        with self.assertRaises(Fail):
            self.retire(yes=False)
        self.m["stop_sessions"].assert_not_called()
        self.m["remove"].assert_not_called()

    def test_no_yes_change_after_kill_keeps_workspace(self):
        def kill(*a, **k):
            self.at = "ccc\n"
            return [("s", "sid")]
        self.m["stop_sessions"].side_effect = kill
        with self.assertRaises(Fail) as c:
            self.retire(yes=False)
        self.assertIn("changed while removing", str(c.exception))
        self.m["remove"].assert_not_called()

    def test_at_recorded_before_stack_merged(self):
        def merged(d):
            self.at = "bbb\n"  # changes while the stack check runs
            return True
        self.m["stack_merged"].side_effect = merged
        for yes in (True, False):
            with self.subTest(yes=yes):
                self.at = "aaa\n"
                with self.assertRaises(Fail) as c:
                    self.retire(yes=yes)
                self.assertIn("changed while removing", str(c.exception))
                self.m["remove"].assert_not_called()

    def test_failure_carries_killed(self):
        def ask(*a, **k):
            self.at = "bbb\n"
        self.m["ask_lost"].side_effect = ask
        with self.assertRaises(Fail) as c:
            self.retire(yes=True)
        self.assertEqual(c.exception.killed, [("s", "sid")])

    def test_oserror_carries_killed(self):
        self.m["remove"].side_effect = OSError("busy")
        with self.assertRaises(OSError) as c:
            self.retire(yes=True)
        self.assertEqual(c.exception.killed, [("s", "sid")])

    def test_ask_lost_failure_after_kill_carries_killed(self):
        self.m["ask_lost"].side_effect = Fail("nothing removed")
        with self.assertRaises(Fail) as c:
            self.retire(yes=True)
        self.assertEqual(c.exception.killed, [("s", "sid")])

    def test_heads_log_before_remove(self):
        self.heads = "abc something\n"
        self.retire()
        o = self.order()
        self.assertLess(o.index("heads"), o.index("remove"))
        self.assertIn("abc something", sys.stderr.getvalue())

    def test_heads_failure_does_not_block_removal(self):
        self.heads_fail = True
        self.assertEqual(self.retire(), [("s", "sid")])
        self.m["remove"].assert_called_once_with("/h", "w")

    def test_jj_calls_use_workspace_cwd(self):
        self.retire(yes=True)
        self.assertTrue(self.jj_calls)
        for a, k in self.jj_calls:
            self.assertEqual(k.get("cwd"), self.D, a)


class CmdPrune(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = tmp.name
        os.makedirs(f"{self.root}/r.agents/ws/.jj")
        self.m = {}
        for n, kw in [("ROOT", {"new": self.root}),
                      ("repos", {"return_value": ["r"]}),
                      ("jj", {"return_value": "main\n"}),
                      ("recent", {"return_value": False}),
                      ("in_use", {"return_value": False}),
                      ("loads", {"return_value": True})]:
            p = mock.patch.object(jw, n, **kw)
            self.m[n] = p.start()
            self.addCleanup(p.stop)
        p = mock.patch.object(shutil, "which", return_value=None)
        p.start()
        self.addCleanup(p.stop)
        self.err = io.StringIO()
        p = mock.patch.object(sys, "stderr", self.err)
        p.start()
        self.addCleanup(p.stop)

    def prune(self, yes):
        jw.cmd_prune(type("A", (), {"yes": yes})())

    def test_yes_reaches_retire(self):
        with mock.patch.object(jw, "retire", return_value=[]) as r:
            self.prune(True)
        r.assert_called_once()
        a, k = r.call_args
        self.assertEqual(a[:2], (f"{self.root}/r", "ws"))
        self.assertIs(k["yes"], True)
        self.assertEqual(k["idle"], 3600)

    def test_no_yes(self):
        with mock.patch.object(jw, "retire", return_value=[]) as r:
            self.prune(False)
        self.assertIs(r.call_args[1]["yes"], False)

    def test_fail_prints_kept(self):
        with mock.patch.object(jw, "retire", side_effect=Fail("unmerged work")):
            self.prune(True)
        self.assertIn("kept", self.err.getvalue())
        self.assertIn("unmerged work", self.err.getvalue())

    def test_yes_keeps_broken(self):
        self.m["loads"].return_value = False
        with mock.patch.object(jw, "retire", return_value=[]) as r, \
                mock.patch.object(jw, "remove_broken") as rb, \
                mock.patch.object(jw, "confirm") as c:
            self.prune(True)
        rb.assert_not_called()
        c.assert_not_called()
        r.assert_not_called()
        self.assertIn("jw: kept ", self.err.getvalue())
        self.assertIn("r.agents/ws", self.err.getvalue())

    def test_no_yes_confirms_and_deletes_broken(self):
        self.m["loads"].return_value = False
        with mock.patch.object(jw, "remove_broken") as rb, \
                mock.patch.object(jw, "confirm") as c:
            self.prune(False)
        c.assert_called_once()
        rb.assert_called_once()

    def test_no_yes_declined_keeps_broken(self):
        self.m["loads"].return_value = False
        with mock.patch.object(jw, "remove_broken") as rb, \
                mock.patch.object(jw, "confirm", side_effect=Fail("nothing removed")):
            self.prune(False)
        rb.assert_not_called()

    def test_no_fetch(self):
        with mock.patch.object(jw, "retire", return_value=[]):
            self.prune(True)
        for a, k in self.m["jj"].call_args_list:
            self.assertNotIn("fetch", a)

    def test_killed_printed_on_success(self):
        with mock.patch.object(jw, "retire", return_value=[("shp1", "sid")]):
            self.prune(True)
        self.assertIn("removed", self.err.getvalue())
        self.assertIn("shp1", self.err.getvalue())

    def test_killed_printed_on_failure(self):
        e = Fail("changed while removing")
        e.killed = [("shp1", "sid")]
        with mock.patch.object(jw, "retire", side_effect=e):
            self.prune(True)
        self.assertIn("kept", self.err.getvalue())
        self.assertIn("changed while removing", self.err.getvalue())
        self.assertIn("shp1", self.err.getvalue())

    def test_killed_printed_on_oserror(self):
        e = OSError("busy")
        e.killed = [("shp1", "sid")]
        with mock.patch.object(jw, "retire", side_effect=e):
            self.prune(True)
        self.assertIn("kept", self.err.getvalue())
        self.assertIn("shp1", self.err.getvalue())

    def test_unchecked_is_kept(self):
        with mock.patch.object(jw, "retire", side_effect=jw.Unchecked("couldn't check the stack")):
            self.prune(True)
        self.assertIn("kept", self.err.getvalue())
        self.assertIn("couldn't check the stack", self.err.getvalue())
        self.assertNotIn("removed", self.err.getvalue())


if __name__ == "__main__":
    unittest.main()
