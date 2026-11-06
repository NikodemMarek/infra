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


class RecordMr(unittest.TestCase):
    URL = "https://git.example/g/p/-/merge_requests/7"

    def env(self, **kw):
        env = {k: v for k, v in os.environ.items() if k != "AGENT_SESSION"}
        env.update(kw)
        return mock.patch.dict(os.environ, env, clear=True)

    def test_unset_no_call(self):
        with self.env(), mock.patch.object(jj_upload.subprocess, "run") as run:
            jj_upload.record_mr(self.URL)
        run.assert_not_called()

    def test_empty_no_call(self):
        with self.env(AGENT_SESSION=""), mock.patch.object(jj_upload.subprocess, "run") as run:
            jj_upload.record_mr(self.URL)
        run.assert_not_called()

    def test_set_runs_jw(self):
        with self.env(AGENT_SESSION="s-1"), mock.patch.object(jj_upload.subprocess, "run") as run, \
                mock.patch.object(jj_upload, "info") as info:
            run.return_value = mock.Mock(returncode=0, stderr="")
            jj_upload.record_mr(self.URL)
        run.assert_called_once_with(
            ["jw", "session", "set", "--mr", self.URL],
            stdin=jj_upload.subprocess.DEVNULL,
            capture_output=True,
            text=True,
            timeout=10,
        )
        info.assert_not_called()

    def test_nonzero_reports(self):
        with self.env(AGENT_SESSION="s-1"), mock.patch.object(jj_upload.subprocess, "run") as run, \
                mock.patch.object(jj_upload, "info") as info:
            run.return_value = mock.Mock(returncode=1, stderr="boom happened")
            jj_upload.record_mr(self.URL)
        info.assert_called_once()
        msg = info.call_args[0][0]
        for part in (self.URL, "s-1", "boom happened"):
            self.assertIn(part, msg)

    def test_oserror_reports(self):
        with self.env(AGENT_SESSION="s-1"), \
                mock.patch.object(jj_upload.subprocess, "run", side_effect=OSError("no jw")), \
                mock.patch.object(jj_upload, "info") as info:
            jj_upload.record_mr(self.URL)
        info.assert_called_once()

    def test_timeout_reports(self):
        exc = jj_upload.subprocess.TimeoutExpired(["jw"], 10)
        with self.env(AGENT_SESSION="s-1"), \
                mock.patch.object(jj_upload.subprocess, "run", side_effect=exc), \
                mock.patch.object(jj_upload, "info") as info:
            jj_upload.record_mr(self.URL)
        info.assert_called_once()


def make_commit(change="abcdefghijkl", commit="c1", description="Old title", bookmarks=()):
    return {"change": change, "commit": commit, "parents": [], "bookmarks": list(bookmarks),
            "conflict": False, "immutable": False, "description": description}


def make_mr(state="merged", head="c1", landed=("m1",), source="nm/old-title-abcdefgh",
            url="https://git.example/g/p/-/merge_requests/7", title="Old title", draft=False):
    return {"id": 7, "ref": "!7", "url": url, "state": state, "draft": draft, "title": title,
            "target": "main", "description": "", "head": head, "landed": list(landed),
            "source": source}


class FakeForge:
    def __init__(self, by_branch=None, recent=()):
        self.by_branch = by_branch or {}
        self._recent = list(recent)
        self.find_calls = []

    def find(self, branch):
        self.find_calls.append(branch)
        return self.by_branch.get(branch)

    def recent(self):
        return self._recent


class ForgeRecent(unittest.TestCase):
    GL = {"iid": 3, "web_url": "https://gl/g/p/-/merge_requests/3", "state": "opened",
          "title": "T", "target_branch": "main", "source_branch": "nm/t-abcdefgh", "sha": "s1"}
    GH = {"number": 4, "html_url": "https://gh/o/p/pull/4", "state": "open", "title": "T",
          "base": {"ref": "main"}, "head": {"ref": "nm/t-abcdefgh", "sha": "s1"}}

    def test_gitlab_norm_source(self):
        self.assertEqual(jj_upload.GitLab("h", "g/p").norm(self.GL)["source"], "nm/t-abcdefgh")

    def test_github_norm_source(self):
        self.assertEqual(jj_upload.GitHub("h", "o/p").norm(self.GH)["source"], "nm/t-abcdefgh")

    def test_gitlab_recent_call_normalised_cached(self):
        fg = jj_upload.GitLab("h", "g/p")
        with mock.patch.object(jj_upload, "cli_api", return_value=[self.GL]) as api:
            first = fg.recent()
            second = fg.recent()
        self.assertEqual(api.call_count, 1)
        cmd = api.call_args[0][0]
        self.assertIn(
            "projects/g%2Fp/merge_requests?state=all&order_by=updated_at&sort=desc&per_page=100", cmd)
        self.assertEqual(first, second)
        self.assertEqual(first[0]["source"], "nm/t-abcdefgh")
        self.assertEqual(first[0]["state"], "open")
        self.assertEqual(first[0]["ref"], "!3")

    def test_github_recent_call_normalised_cached(self):
        fg = jj_upload.GitHub("h", "o/p")
        with mock.patch.object(jj_upload, "cli_api", return_value=[self.GH]) as api:
            first = fg.recent()
            fg.recent()
        self.assertEqual(api.call_count, 1)
        self.assertIn("repos/o/p/pulls?state=all&sort=updated&direction=desc&per_page=100",
                      api.call_args[0][0])
        self.assertEqual(first[0]["source"], "nm/t-abcdefgh")
        self.assertEqual(first[0]["ref"], "#4")

    def test_fatal_propagates(self):
        for fg in (jj_upload.GitLab("h", "g/p"), jj_upload.GitHub("h", "o/p")):
            with mock.patch.object(jj_upload, "cli_api", side_effect=jj_upload.Fatal("boom")):
                with self.assertRaises(jj_upload.Fatal):
                    fg.recent()


class FindMr(unittest.TestCase):
    def test_by_branch(self):
        mr = make_mr(source="whatever")
        fg = FakeForge({"nm/x-abcdefgh": mr}, recent=[make_mr(source="nm/other-abcdefgh")])
        self.assertIs(jj_upload.find_mr(fg, make_commit(), "nm/x-abcdefgh", "nm/"), mr)

    def test_by_suffix_after_title_change(self):
        mr = make_mr(source="nm/old-title-abcdefgh")
        fg = FakeForge(recent=[make_mr(source="nm/unrelated-12345678"), mr])
        self.assertIs(jj_upload.find_mr(fg, make_commit(), None, "nm/"), mr)
        self.assertIs(jj_upload.find_mr(fg, make_commit(), "nm/new-title-abcdefgh", "nm/"), mr)

    def test_first_recent_wins(self):
        a = make_mr(source="nm/a-abcdefgh")
        b = make_mr(source="nm/b-abcdefgh")
        fg = FakeForge(recent=[a, b])
        self.assertIs(jj_upload.find_mr(fg, make_commit(), None, "nm/"), a)

    def test_prefix_mismatch(self):
        fg = FakeForge(recent=[make_mr(source="other/old-title-abcdefgh")])
        self.assertIsNone(jj_upload.find_mr(fg, make_commit(), None, "nm/"))

    def test_none_at_all(self):
        self.assertIsNone(jj_upload.find_mr(FakeForge(), make_commit(), "nm/x-abcdefgh", "nm/"))

    def test_falls_back_to_computed_name(self):
        mr = make_mr(source="nm/old-title-abcdefgh")
        fg = FakeForge({"nm/old-title-abcdefgh": mr}, recent=[make_mr(source="nm/unrelated-12345678")])
        self.assertIs(jj_upload.find_mr(fg, make_commit(), "nm/gone-abcdefgh", "nm/"), mr)
        self.assertIs(jj_upload.find_mr(fg, make_commit(), None, "nm/"), mr)

    def test_same_name_not_looked_up_twice(self):
        fg = FakeForge()
        self.assertIsNone(jj_upload.find_mr(fg, make_commit(), "nm/old-title-abcdefgh", "nm/"))
        self.assertEqual(fg.find_calls, ["nm/old-title-abcdefgh"])


class ChangeState(unittest.TestCase):
    def state(self, mr, trunk_ids=("m1",), interdiff="", present=True, commit="c1"):
        def log_ids(revset, field="commit_id"):
            if "::trunk()" in revset:
                return [i for i in trunk_ids if i in revset]
            return ["x"] if present else []

        with mock.patch.object(jj_upload, "log_ids", side_effect=log_ids), \
                mock.patch.object(jj_upload, "jj", return_value=interdiff):
            return jj_upload.change_state(make_commit(commit=commit), mr)

    def test_no_mr(self):
        self.assertEqual(self.state(None), "no-mr")

    def test_open_and_draft(self):
        self.assertEqual(self.state(make_mr(state="open")), "open")
        self.assertEqual(self.state(make_mr(state="open", draft=True)), "open")

    def test_closed(self):
        self.assertEqual(self.state(make_mr(state="closed")), "closed")

    def test_merged(self):
        self.assertEqual(self.state(make_mr()), "merged")

    def test_merged_head_missing(self):
        self.assertEqual(self.state(make_mr(head=None)), "changed")

    def test_merged_head_not_present(self):
        self.assertEqual(self.state(make_mr(), present=False), "changed")

    def test_merged_head_differs_with_interdiff(self):
        self.assertEqual(self.state(make_mr(head="old"), interdiff="M f\n"), "changed")

    def test_merged_head_differs_empty_interdiff(self):
        self.assertEqual(self.state(make_mr(head="old"), trunk_ids=("m1",)), "merged")

    def test_merged_elsewhere(self):
        self.assertEqual(self.state(make_mr(), trunk_ids=()), "elsewhere")

    def test_merged_head_in_trunk_counts(self):
        self.assertEqual(self.state(make_mr(landed=()), trunk_ids=("c1",)), "merged")


class StackStatus(unittest.TestCase):
    def run_status(self, loads, forge=None, fetch_error=None, make_forge_error=None):
        calls = []

        def jj(*args, **kw):
            calls.append(args)
            if args[:2] == ("git", "fetch") and fetch_error:
                raise fetch_error
            return ""

        mf = mock.Mock(return_value=forge)
        if make_forge_error:
            mf.side_effect = make_forge_error
        out, err = io.StringIO(), io.StringIO()
        with mock.patch.object(jj_upload, "load_commits", side_effect=loads) as lc, \
                mock.patch.object(jj_upload, "jj", side_effect=jj), \
                mock.patch.object(jj_upload, "log_ids", return_value=["x"]), \
                mock.patch.object(jj_upload, "make_forge", mf), \
                contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            rc = jj_upload.stack_status("origin", "nm/")
        return rc, out.getvalue().splitlines(), err.getvalue(), calls, mf, lc

    def test_empty_stack_no_fetch(self):
        rc, lines, _, calls, mf, lc = self.run_status([[]])
        self.assertEqual((rc, lines), (0, ["merged"]))
        self.assertNotIn(("git", "fetch"), calls)
        mf.assert_not_called()
        lc.assert_called_once_with(jj_upload.DEFAULT_REVSET)

    def test_all_merged(self):
        cs = [make_commit("aaaaaaaaaaaa", "c1", "First"), make_commit("bbbbbbbbbbbb", "c2", "Second")]
        fg = FakeForge(recent=[
            make_mr(head="c1", source="nm/first-aaaaaaaa", url="u1", title="First"),
            make_mr(head="c2", source="nm/second-bbbbbbbb", url="u2", title="Second"),
        ])
        rc, lines, _, calls, _, _ = self.run_status([cs, cs], fg)
        self.assertEqual(rc, 0)
        self.assertEqual(lines, ["aaaaaaaa\tmerged\tu1\tFirst", "bbbbbbbb\tmerged\tu2\tSecond", "merged"])
        self.assertIn(("git", "fetch"), calls)

    def test_nothing_left_after_fetch(self):
        cs = [make_commit()]
        rc, lines, _, calls, _, _ = self.run_status([cs, []], FakeForge())
        self.assertEqual((rc, lines[-1]), (0, "merged"))
        self.assertIn(("git", "fetch"), calls)

    def test_one_open_and_line_format(self):
        cs = [make_commit("aaaaaaaaaaaa", "c1", "First"), make_commit("bbbbbbbbbbbb", "c2", "Second"),
              make_commit("cccccccccccc", "c3", "Third")]
        fg = FakeForge(recent=[
            make_mr(head="c1", source="nm/first-aaaaaaaa", url="u1", title="First"),
            make_mr(state="open", head="c2", source="nm/second-bbbbbbbb", url="u2", title="Second"),
        ])
        rc, lines, _, _, _, _ = self.run_status([cs, cs], fg)
        self.assertEqual(rc, 1)
        self.assertEqual(lines, ["aaaaaaaa\tmerged\tu1\tFirst", "bbbbbbbb\topen\tu2\tSecond",
                                 "cccccccc\tno-mr\t-\tThird", "not merged"])

    def test_fetch_fatal_is_unknown(self):
        rc, lines, err, _, _, _ = self.run_status(
            [[make_commit()]], FakeForge(), fetch_error=jj_upload.Fatal("fetch failed"))
        self.assertEqual(rc, 2)
        self.assertEqual(lines[-1], "unknown")
        self.assertIn("fetch failed", err)

    def test_make_forge_fatal_is_unknown(self):
        rc, lines, err, _, _, _ = self.run_status(
            [[make_commit()], [make_commit()]], make_forge_error=jj_upload.Fatal("no forge"))
        self.assertEqual((rc, lines[-1]), (2, "unknown"))
        self.assertIn("no forge", err)

    def test_forge_cli_fatal_is_unknown(self):
        class Boom(FakeForge):
            def find(self, branch):
                raise jj_upload.Fatal("glab failed")

            def recent(self):
                raise jj_upload.Fatal("glab failed")

        rc, lines, err, _, _, _ = self.run_status([[make_commit()], [make_commit()]], Boom())
        self.assertEqual((rc, lines[-1]), (2, "unknown"))
        self.assertIn("glab failed", err)

    def test_names_come_from_pre_fetch_load(self):
        before = [make_commit(bookmarks=["nm/custom-name-abcdefgh"])]
        after = [make_commit()]
        fg = FakeForge({"nm/custom-name-abcdefgh": make_mr(source="nm/custom-name-abcdefgh", url="u1")})
        rc, lines, _, _, _, _ = self.run_status([before, after], fg)
        self.assertEqual((rc, lines[-1]), (0, "merged"))
        self.assertEqual(lines[0].split("\t")[1], "merged")

    def two_commit_status(self, first, second=None):
        cs = [make_commit("aaaaaaaaaaaa", "c1", "First"), make_commit("bbbbbbbbbbbb", "c2", "Second")]
        recent = [make_mr(**{"head": "c1", "source": "nm/first-aaaaaaaa", "url": "u1", "title": "First",
                             **first})]
        if second is not None:
            recent.append(make_mr(**{"head": "c2", "source": "nm/second-bbbbbbbb", "url": "u2",
                                     "title": "Second", **second}))
        return self.run_status([cs, cs], FakeForge(recent=recent))

    def test_closed_is_not_merged(self):
        rc, lines, _, _, _, _ = self.two_commit_status({"state": "closed"})
        self.assertEqual((rc, lines[0].split("\t")[1], lines[-1]), (1, "closed", "not merged"))

    def test_changed_is_not_merged(self):
        rc, lines, _, _, _, _ = self.two_commit_status({"head": None})
        self.assertEqual((rc, lines[0].split("\t")[1], lines[-1]), (1, "changed", "not merged"))

    def test_elsewhere_is_not_merged(self):
        cs = [make_commit()]
        out, err = io.StringIO(), io.StringIO()

        def log_ids(revset, field="commit_id"):
            return [] if "::trunk()" in revset else ["x"]

        with mock.patch.object(jj_upload, "load_commits", side_effect=[cs, cs]), \
                mock.patch.object(jj_upload, "jj", return_value=""), \
                mock.patch.object(jj_upload, "log_ids", side_effect=log_ids), \
                mock.patch.object(jj_upload, "make_forge", return_value=FakeForge(recent=[make_mr()])), \
                contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            rc = jj_upload.stack_status("origin", "nm/")
        lines = out.getvalue().splitlines()
        self.assertEqual((rc, lines[0].split("\t")[1], lines[-1]), (1, "elsewhere", "not merged"))

    def test_merged_and_changed_is_not_merged(self):
        rc, lines, _, _, _, _ = self.two_commit_status({}, {"head": None})
        self.assertEqual([ln.split("\t")[1] for ln in lines[:2]], ["merged", "changed"])
        self.assertEqual((rc, lines[-1]), (1, "not merged"))

    def test_unexpected_error_is_unknown(self):
        class Boom(FakeForge):
            def find(self, branch):
                raise ValueError("bad payload")

            def recent(self):
                raise ValueError("bad payload")

        rc, lines, err, _, _, _ = self.run_status([[make_commit()], [make_commit()]], Boom())
        self.assertEqual((rc, lines[-1]), (2, "unknown"))
        self.assertTrue(err.strip())


class StackDispatch(unittest.TestCase):
    class Stop(Exception):
        pass

    def run_main(self, argv, config=None):
        def cfg(key, default=None):
            if config is None:
                raise AssertionError("jj config looked up before the stack dispatch")
            return config.get(key, default)

        code = None
        with mock.patch.object(jj_upload.sys, "argv", ["jj-upload", *argv]), \
                mock.patch.object(jj_upload, "jj_config", side_effect=cfg), \
                mock.patch.object(jj_upload.os, "execvp", side_effect=self.Stop) as ex, \
                mock.patch.object(jj_upload, "stack_status", return_value=1) as ss:
            try:
                jj_upload.main()
            except self.Stop:
                code = "exec"
            except SystemExit as e:
                code = e.code
        return code, ex, ss

    def test_plain_stack_execs_jj_log(self):
        code, ex, ss = self.run_main(["stack"])
        self.assertEqual(code, "exec")
        ex.assert_called_once_with("jj", ["jj", "log", "-r", "stack()"])
        ss.assert_not_called()

    def test_stack_passes_extra_args(self):
        code, ex, _ = self.run_main(["stack", "--no-graph", "-T", "x"])
        self.assertEqual(code, "exec")
        ex.assert_called_once_with("jj", ["jj", "log", "-r", "stack()", "--no-graph", "-T", "x"])

    def test_status_exits_with_return_code(self):
        code, ex, ss = self.run_main(
            ["stack", "status"], {"upload.remote": "up", "upload.bookmark-prefix": "nm/"})
        self.assertEqual(code, 1)
        ss.assert_called_once_with("up", "nm/")
        ex.assert_not_called()


class MergedBottomSuffix(unittest.TestCase):
    def test_merged_found_by_suffix(self):
        c = make_commit(description="New title")
        fg = FakeForge(recent=[make_mr(source="nm/old-title-abcdefgh")])
        with mock.patch.object(jj_upload, "make_forge", return_value=fg), \
                mock.patch.object(jj_upload, "log_ids", return_value=["x"]), \
                mock.patch.object(jj_upload, "jj", return_value=""):
            self.assertEqual(jj_upload.merged_bottom("origin", "nm/", [c], {}, {}), [c])

    def test_fatal_returns_empty(self):
        with mock.patch.object(jj_upload, "make_forge", side_effect=jj_upload.Fatal("x")), \
                mock.patch.object(jj_upload, "info"):
            self.assertEqual(jj_upload.merged_bottom("origin", "nm/", [make_commit()], {}, {}), [])

    def test_open_middle_returns_only_first(self):
        cs = [make_commit("aaaaaaaaaaaa", "c1", "First"), make_commit("bbbbbbbbbbbb", "c2", "Second"),
              make_commit("cccccccccccc", "c3", "Third")]
        fg = FakeForge(recent=[
            make_mr(head="c1", source="nm/first-aaaaaaaa"),
            make_mr(state="open", head="c2", source="nm/second-bbbbbbbb"),
            make_mr(head="c3", source="nm/third-cccccccc"),
        ])
        with mock.patch.object(jj_upload, "make_forge", return_value=fg), \
                mock.patch.object(jj_upload, "log_ids", return_value=["x"]), \
                mock.patch.object(jj_upload, "jj", return_value=""):
            self.assertEqual(jj_upload.merged_bottom("origin", "nm/", cs, {}, {}), [cs[0]])

    def test_remote_branch_preferred_over_names(self):
        c = make_commit()
        with mock.patch.object(jj_upload, "make_forge", return_value=FakeForge()), \
                mock.patch.object(jj_upload, "find_mr", return_value=None) as fm:
            jj_upload.merged_bottom("origin", "nm/", [c], {c["change"]: "nm/local-abcdefgh"},
                                    {c["change"]: ("nm/remote-abcdefgh", "tip")})
        self.assertEqual(fm.call_args[0][2], "nm/remote-abcdefgh")


class CliApi(unittest.TestCase):
    def test_non_json_output_is_fatal(self):
        with tempfile.TemporaryDirectory() as d:
            script = os.path.join(d, "glab")
            with open(script, "w") as f:
                f.write("#!/bin/sh\necho 'not json at all'\n")
            os.chmod(script, 0o755)
            with self.assertRaises(jj_upload.Fatal):
                jj_upload.cli_api([script, "api", "x"])


if __name__ == "__main__":
    unittest.main()
