import contextlib
import io
import json
import os
import tempfile
import unittest
from unittest import mock

import jj_upload
from jj_upload import record_path, record_session, record_upload, repo_name


class RepoName(unittest.TestCase):
    def test_plain(self):
        self.assertEqual(repo_name("/p/proj"), "proj")

    def test_agent_workspace(self):
        self.assertEqual(repo_name("/p/proj.agents/s-1"), "proj")


class RecordPath(unittest.TestCase):
    def test_encodes_slash(self):
        self.assertEqual(
            record_path("/s", "proj", "nm1/feat-x-abcdefgh"),
            "/s/jj-upload/proj/nm1%2Ffeat-x-abcdefgh.json",
        )


class RecordUpload(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)

    def read(self, state_home, repo, branch):
        with open(record_path(state_home, repo, branch)) as f:
            return json.load(f)

    def test_writes_record_creating_dirs(self):
        record_upload("nm1/feat-x", "proj", "/w/proj.agents/s-1", "sid", 1767225600.0, state_home=self.tmp.name)
        self.assertEqual(
            self.read(self.tmp.name, "proj", "nm1/feat-x"),
            {"workspace": "/w/proj.agents/s-1", "session_id": "sid", "uploaded_at": 1767225600.0},
        )

    def test_overwrites(self):
        record_upload("b", "proj", "w1", "s1", 1.0, state_home=self.tmp.name)
        record_upload("b", "proj", "w2", "s2", 2.0, state_home=self.tmp.name)
        self.assertEqual(
            self.read(self.tmp.name, "proj", "b"),
            {"workspace": "w2", "session_id": "s2", "uploaded_at": 2.0},
        )

    def test_default_xdg_state_home(self):
        with mock.patch.dict(os.environ, {"XDG_STATE_HOME": self.tmp.name}):
            record_upload("b", "proj", "w", "s", 3.0)
        self.assertEqual(self.read(self.tmp.name, "proj", "b")["session_id"], "s")

    def test_default_home_fallback(self):
        env = {k: v for k, v in os.environ.items() if k != "XDG_STATE_HOME"}
        env["HOME"] = self.tmp.name
        with mock.patch.dict(os.environ, env, clear=True):
            record_upload("b", "proj", "w", "s", 3.0)
        self.assertEqual(self.read(os.path.join(self.tmp.name, ".local", "state"), "proj", "b")["workspace"], "w")

    def test_oserror_reports_and_does_not_raise(self):
        bad = os.path.join(self.tmp.name, "file")
        with open(bad, "w") as f:
            f.write("x")
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            record_upload("br", "proj", "w", "s", 3.0, state_home=bad)
        self.assertTrue(
            err.getvalue().startswith("jj upload: cannot record the session for br:"),
            err.getvalue(),
        )


class RecordSession(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.state = os.path.join(self.tmp.name, "state")
        self.root = os.path.join(self.tmp.name, "proj.agents", "s-1")
        os.makedirs(self.root)
        jj_upload.workspace_root.cache_clear()
        self.addCleanup(jj_upload.workspace_root.cache_clear)

    def env(self, **kw):
        env = {k: v for k, v in os.environ.items() if k != "CLAUDE_CODE_SESSION_ID"}
        env.update(kw)
        return mock.patch.dict(os.environ, env, clear=True)

    def files(self):
        return [f for _, _, fs in os.walk(self.state) for f in fs]

    def test_unset_writes_nothing(self):
        with self.env(), mock.patch.object(jj_upload, "jj", return_value=self.root + "\n"):
            record_session("b", now=1.0, state_home=self.state)
        self.assertEqual(self.files(), [])

    def test_empty_writes_nothing(self):
        with self.env(CLAUDE_CODE_SESSION_ID=""), mock.patch.object(jj_upload, "jj", return_value=self.root + "\n"):
            record_session("b", now=1.0, state_home=self.state)
        self.assertEqual(self.files(), [])

    def test_writes_record(self):
        link = os.path.join(self.tmp.name, "link")
        os.symlink(self.root, link)
        with self.env(CLAUDE_CODE_SESSION_ID="sid-1"), mock.patch.object(jj_upload, "jj", return_value=link + "\n"):
            record_session("nm1/feat-x", now=1767225600.0, state_home=self.state)
        real = os.path.realpath(link)
        with open(record_path(self.state, repo_name(real), "nm1/feat-x")) as f:
            self.assertEqual(
                json.load(f),
                {"workspace": real, "session_id": "sid-1", "uploaded_at": 1767225600.0},
            )

    def test_jj_root_failure_reports_and_does_not_raise(self):
        err = io.StringIO()
        with self.env(CLAUDE_CODE_SESSION_ID="sid"), \
                mock.patch.object(jj_upload, "jj", side_effect=jj_upload.Fatal("no repo")), \
                contextlib.redirect_stderr(err):
            record_session("br", now=1.0, state_home=self.state)
        self.assertTrue(
            err.getvalue().startswith("jj upload: cannot record the session for br:"),
            err.getvalue(),
        )
        self.assertEqual(self.files(), [])


if __name__ == "__main__":
    unittest.main()
