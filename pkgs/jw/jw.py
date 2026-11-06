#!/usr/bin/env python3
"""jw: quick jj workspaces for the repos under one root (~/projects unless the package sets another), at <repo>.agents/<name> (the layout the claude wrapper uses too).

new, cd, rm and clone print the directory to go to on stdout; the `jw` fish function cds there.
pick prints the chosen repo's name on stdout. Everything else goes to stderr.

agent always starts Claude in a new workspace (s-*, like `jw new`) in a shpool session of the same name.
resume continues one of the repo's earlier Claude sessions the same way, in the workspace it ran in.
"""

import argparse
import contextlib
import fcntl
import json
import os
import re
import shlex
import shutil
import socket
import stat
import subprocess
import sys
import tempfile
import time
import unicodedata

ROOT = os.path.expanduser("~/projects")
FZF = "fzf"  # the nix package pins the store paths
SHPOOL = "shpool"
BASH = "bash"
DIRENV = "direnv"
CKSUM = "cksum"
KNOWLEDGE = "knowledge"  # the nix package pins it
SESSION_NAME = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]*")  # fullmatch
GITLAB = re.compile(r"^gl-[a-z]+-[0-9]+(-[0-9]+)?$")  # gitlab-context's workspaces: it reuses and cleans them
IDLE_LIMIT = 4 * 3600  # seconds a detached Claude session must have been idle to be reaped
FINISHED_IDLE = 600  # seconds a finished session must have been idle and detached to be stopped
REAP_DEADLINE = 600  # seconds the reaper works through the session records, so the run ends within its timeout
FINISH_MARKS = ("status", "status_at", "waiting_for", "notified", "reaped")  # record keys of the state of a session (done or not) and its reaping; a new session start drops them
WRAP_UP = ("The merge requests of this session are merged. Finish what is left of this work "
           "(e.g. record the outcome), then run `jw done`. The session is stopped once it is idle "
           "and nobody is attached.")
MSG_WAIT = 10  # seconds `jw msg` waits for a session to register
MSG_MAX = 8000  # characters of a message delivered
RECORD_MAX = 65536  # bytes of a session record that are read
RECORD_DAYS = 7  # age in days after which creating a record deletes the untouched ones
LOCK_WAIT = 5  # seconds a record update waits for the records lock
HTTPS = re.compile(r"https://[!-~]+")  # fullmatch
URL_MAX = 2048


class Fail(Exception):
    pass


class Unchecked(Fail):
    """The stack's merge state couldn't be checked; retry later."""


def jj(*args, cwd=None):
    """Run jj; its own messages and errors go straight to the terminal."""
    p = subprocess.run(["jj", "--no-pager", *args], cwd=cwd, stdout=subprocess.PIPE, text=True)
    if p.returncode != 0:
        raise Fail()  # jj already said why
    return p.stdout


def layout(root):
    """(repo, workspace name, home = root of the main workspace) of the workspace at root."""
    parent = os.path.dirname(root)
    if re.search(r".\.agents$", parent):
        return os.path.basename(parent).removesuffix(".agents"), os.path.basename(root), parent.removesuffix(".agents")
    return os.path.basename(root), "default", root


def locate():
    """(root of the current workspace, home = root of the main workspace)."""
    p = subprocess.run(["jj", "--ignore-working-copy", "workspace", "root"],
                       capture_output=True, text=True)
    if p.returncode != 0:
        raise Fail("not in a jj repo")
    root = p.stdout.strip()
    return root, layout(root)[2]


def repos():
    """Sorted names of the jj repos directly in ROOT."""
    try:
        entries = list(os.scandir(ROOT))
    except FileNotFoundError:
        return []
    return sorted(e.name for e in entries
                  if not e.name.startswith(".") and not e.name.endswith(".agents")
                  and os.path.isdir(f"{e.path}/.jj"))


def workspaces(first=None):
    """Labels (repo/name) of the workspaces in the <repo>.agents dirs, those of the repo `first` before the others."""
    items = []
    for r in repos():
        try:
            names = sorted(os.listdir(f"{ROOT}/{r}.agents"))
        except OSError:
            continue
        items += [f"{r}/{n}" for n in names if os.path.isdir(f"{ROOT}/{r}.agents/{n}/.jj")]
    return sorted(items, key=lambda i: i.split("/")[0] != first)  # stable


def pick(items, query="", empty=None):
    """Let the user choose with fzf (it draws on /dev/tty, so the fish function can capture stdout)."""
    if not items:
        raise Fail(empty or f"no jj repos under {ROOT}")
    p = subprocess.run([FZF, "--select-1", "--exit-0", "--query", query],
                       input="\n".join(items), stdout=subprocess.PIPE, text=True)
    if p.returncode == 1:
        raise Fail("no match")
    if p.returncode == 130:  # Esc, Ctrl-C
        raise Fail("cancelled")  # `jw pick` turns it into exit 130, which agent-inbox treats as cancelled
    if p.returncode != 0:
        raise Fail(f"fzf failed (exit {p.returncode})")
    return p.stdout.strip()


def repo_home(arg):
    """The repo to act on: the named one, else a picked one (the current repo listed first)."""
    if arg and os.path.isdir(f"{ROOT}/{arg}/.jj"):
        return f"{ROOT}/{arg}"
    items = repos()
    try:
        current = os.path.relpath(locate()[1], ROOT)
        if current in items:
            items.remove(current)
            items.insert(0, current)
    except Fail:
        pass  # not in a repo
    return f"{ROOT}/{pick(items, arg or '')}"


def route(task):
    """Guess the repo for the task with `knowledge route`: (repo, reason), repo None = no confident guess."""
    names = repos()
    if not names:
        return None, f"no jj repos under {ROOT}"
    sys.stderr.write("jw: choosing a repo...\n")
    env = {k: v for k, v in os.environ.items() if k != "CLAUDECODE"}  # a nested claude refuses to start otherwise
    try:
        p = subprocess.run([KNOWLEDGE, "route", "--", task], input="\n".join(f"{ROOT}/{n}" for n in names),
                           env=env, capture_output=True, text=True, timeout=90)
        if p.returncode != 0:
            last = next((l.strip() for l in reversed(p.stderr.splitlines()) if l.strip()), "")
            raise OSError(last or f"exit {p.returncode}")
    except (OSError, subprocess.TimeoutExpired) as e:
        why = "timed out" if isinstance(e, subprocess.TimeoutExpired) else e
        return None, f"routing failed: {why}"
    try:
        answer = json.loads(p.stdout)
        repo, reason = answer["repo"], answer.get("reason", "")
    except (ValueError, TypeError, KeyError):  # not JSON, not an object, or no repo key
        repo, reason = None, "bad answer"
    if repo not in names:
        return None, reason or "no clear match"
    sys.stderr.write(f"jw: {repo}: {reason}\n")
    return repo, reason


def in_use(d, ignore=()):
    """Whether some process, other than those in ignore (pids), has its working directory in d."""
    d = os.path.realpath(d)
    for pid in os.listdir("/proc"):
        if not pid.isdigit() or int(pid) in ignore:
            continue
        try:
            cwd = os.readlink(f"/proc/{pid}/cwd")
        except OSError:
            continue
        if cwd == d or cwd.startswith(d + "/"):
            return True
    return False


def fetch(home):
    try:
        jj("--ignore-working-copy", "git", "fetch", cwd=home)
    except Fail:
        sys.stderr.write("jw: fetch failed, continuing\n")


def add_workspace(home, revision, name, resolve_in):
    """Create the workspace <home>.agents/<name> on revision and return its directory. resolve_in is the workspace
    directory to resolve revision in (its edits are snapshotted first), None for home without snapshotting."""
    # resolve first: `workspace add` leaves its directory behind on a bad revision
    flags = ["--ignore-working-copy"] if resolve_in is None else []
    commits = jj(*flags, "log", "--no-graph", "-r", revision,
                 "-T", 'if(root, "root", commit_id) ++ "\\n"', cwd=resolve_in or home).split()
    if len(commits) != 1:
        raise Fail(f"{revision!r} must be exactly one commit, not {len(commits)}")
    if commits[0] == "root":
        raise Fail(f"{revision} resolves to the root commit; pass -r")
    d = f"{home}.agents/{name}"
    os.makedirs(os.path.dirname(d), exist_ok=True)
    jj("workspace", "add", "--name", name, "-r", commits[0], d, cwd=home)
    return d


def resolve_dir(home):
    """The current workspace's root if it belongs to the repo at home (a revision resolves there), else None."""
    try:
        root, current_home = locate()
    except Fail:
        return None  # not in a repo
    return root if os.path.realpath(current_home) == os.path.realpath(home) else None


def cmd_new(a):
    home = repo_home(a.repo)
    fetch(home)
    # the default trunk() is resolved in home without a snapshot
    resolve_in = resolve_dir(home) if a.revision else None
    print(add_workspace(home, a.revision or "trunk()", time.strftime("w-%m%d-%H%M%S"), resolve_in))


def cmd_cd(a):
    current = None
    try:
        root, home = locate()
        if a.name and os.path.isdir(f"{home}.agents/{a.name}"):
            print(f"{home}.agents/{a.name}")
            return
        current = layout(root)[0]
    except Fail:
        pass  # not in a repo
    repo, name = pick(workspaces(current), a.name or "", "no workspaces").split("/", 1)
    print(f"{ROOT}/{repo}.agents/{name}")


def cmd_clone(a):
    name = a.name or re.split(r"[/:]", a.url.rstrip("/"))[-1].removesuffix(".git")
    if not SESSION_NAME.fullmatch(name) or name.endswith(".agents"):
        raise Fail(f"invalid name {name!r}")
    d = f"{ROOT}/{name}"
    if os.path.lexists(d):
        raise Fail(f"{d} already exists")
    os.makedirs(ROOT, exist_ok=True)
    jj("git", "clone", "--colocate", a.url, d)
    print(d)


def untracked(d):
    """Files in d that jj does not track (ignored or too large): they die with the directory."""
    on_disk = set()
    for path, dirs, files in os.walk(d):
        if ".jj" in dirs:
            dirs.remove(".jj")
        if path == d:  # jj may put a .git file in the workspaces of colocated repos
            dirs[:] = [x for x in dirs if x != ".git"]
            files = [x for x in files if x != ".git"]
        links = [x for x in dirs if os.path.islink(f"{path}/{x}")]  # jj tracks a symlink as one file
        dirs[:] = [x for x in dirs if x not in links]
        on_disk.update(os.path.relpath(f"{path}/{x}", d) for x in files + links)
    return sorted(on_disk - set(jj("--ignore-working-copy", "file", "list", cwd=d).splitlines()))


def confirm(question, yes=False):
    """Ask on the terminal; Fail unless the answer is y. With yes, answer y without asking."""
    if yes:
        sys.stderr.write(f"{question}y (--yes)\n")
        return
    sys.stderr.write(question)
    sys.stderr.flush()  # the question has no newline, and stderr is line-buffered
    try:
        with open("/dev/tty") as tty:  # stdout is captured by the fish function
            answer = tty.readline()
    except OSError:
        raise Fail("nothing removed (no terminal to confirm)")
    if answer.strip().lower() != "y":
        raise Fail("nothing removed")


def rmdir_agents(home):
    """Remove <repo>.agents if it is empty now."""
    try:
        os.rmdir(f"{home}.agents")
    except OSError:
        pass


def collapse(paths, many=10):
    """paths, with each top-level directory that holds more than many of them as one `dir/ (N files)`."""
    count = {}
    for p in paths:
        top = p.split("/", 1)[0]
        count[top] = count.get(top, 0) + 1
    shown = {}
    for p in paths:
        top = p.split("/", 1)[0]
        shown[f"{top}/ ({count[top]} files)" if count[top] > many and top != p else p] = None
    return list(shown)


def ask_lost(d, yes=False, snapshot=True, asked=()):
    """Snapshot the workspace at d (unless snapshot is false) and, if files jj does not track would be lost with it
    and are not all in asked, ask whether to delete. Return the files that would be lost."""
    if snapshot:
        jj("status", cwd=d)  # snapshot: its work stays in its change after the workspace is gone
    lost = untracked(d)
    if set(lost) - set(asked):
        if yes:
            listed = ", ".join(collapse(lost))
        else:
            more = f", ... (+{len(lost) - 5} more)" if len(lost) > 5 else ""
            listed = f"{', '.join(lost[:5])}{more}"
        confirm(f"jw: {len(lost)} file(s) in {d} are not tracked by jj (ignored or too large) "
                f"and will be lost: {listed}. Delete? [y/N] ", yes)
    return lost


def remove(home, name):
    d = f"{home}.agents/{name}"
    jj("--ignore-working-copy", "workspace", "forget", cwd=d)  # its own workspace: the jj name may differ from name
    shutil.rmtree(d)
    rmdir_agents(home)


def remove_broken(home, name, known):
    """Delete a workspace jj cannot read (the caller has asked: jj cannot tell what is in there)."""
    if name in known:
        try:
            jj("--ignore-working-copy", "workspace", "forget", name, cwd=home)
        except Fail:
            pass  # its directory goes anyway
    else:  # a renamed workspace: the jj name is unknown without reading the directory
        sys.stderr.write(f"jw: the jj workspace for {name} may remain; see 'jj workspace list' / 'jj workspace forget <name>'\n")
    shutil.rmtree(f"{home}.agents/{name}")
    rmdir_agents(home)


def recent(d):
    """Whether d was modified within the last hour (a running agent may not have edited anything yet)."""
    mtimes = []
    for p in (d, f"{d}/.jj/working_copy"):
        try:
            mtimes.append(os.path.getmtime(p))
        except OSError:
            pass
    return bool(mtimes) and time.time() - max(mtimes) < 3600


def loads(d):
    """Whether jj can load the workspace at d."""
    p = subprocess.run(["jj", "--no-pager", "--ignore-working-copy", "log", "--no-graph", "-T", "", "-r", "@"],
                       cwd=d, capture_output=True)
    return p.returncode == 0


def cmd_rm(a):
    root, home = locate()
    name = a.name or (os.path.basename(root) if root != home else None)
    if not name:
        raise Fail("give a workspace name (this is the main one)")
    d = f"{home}.agents/{name}"
    if name == "default" or not SESSION_NAME.fullmatch(name) or not os.path.isdir(f"{d}/.jj"):
        raise Fail(f"no workspace at {d}")
    inside = (os.path.realpath(os.getcwd()) + "/").startswith(os.path.realpath(d) + "/")
    ask_lost(d)
    remove(home, name)
    if inside:
        print(home)


def stat_fields(pid, proc="/proc"):
    """Fields 3.. of /proc/<pid>/stat (the command name, field 2, may contain spaces and parentheses): ppid is
    [1], the start time (field 22) [19]."""
    with open(f"{proc}/{pid}/stat") as f:
        return f.read().rpartition(")")[2].split()


def claude_dir():
    return os.environ.get("CLAUDE_CONFIG_DIR") or os.path.expanduser("~/.local/share/claude")


def live_claudes():
    """[(pid, session file contents, working directory, shpool session or None)] of the live Claude sessions."""
    base = f"{claude_dir()}/sessions"
    try:
        files = os.listdir(base)
    except OSError:
        return []
    found = []
    for f in files:
        try:
            with open(f"{base}/{f}") as fh:
                s = json.load(fh)
            pid = int(s["pid"])
            if stat_fields(pid)[19] != s["procStart"]:  # a dead pid fails here
                continue  # the pid was reused
            cwd = os.readlink(f"/proc/{pid}/cwd")
            with open(f"/proc/{pid}/environ", "rb") as fh:
                env = fh.read().split(b"\0")
        except (OSError, ValueError, KeyError, TypeError, IndexError):
            continue
        prefix = b"SHPOOL_SESSION_NAME="
        name = next((e[len(prefix):].decode(errors="replace") for e in env if e.startswith(prefix)), None)
        found.append((pid, s, cwd, name))
    return found


def claude_sessions(d):
    """{pid: (status, shpool session or None, statusUpdatedAt in ms or None, sessionId)} of the live Claude sessions
    with their working directory in d."""
    d = os.path.realpath(d)
    return {pid: (s.get("status"), name, s.get("statusUpdatedAt"), s.get("sessionId", "?"))
            for pid, s, cwd, name in live_claudes()
            if cwd == d or cwd.startswith(d + "/")}


def descendants(pids):
    """pids and all their descendant processes."""
    children = {}
    for p in os.listdir("/proc"):
        if p.isdigit():
            try:
                ppid = int(stat_fields(p)[1])
            except (OSError, ValueError, IndexError):
                continue
            children.setdefault(ppid, []).append(int(p))
    found, todo = set(), list(pids)
    while todo:
        p = todo.pop()
        if p not in found:
            found.add(p)
            todo += children.get(p, [])
    return found


def shpool_status():
    """{shpool session: attached or disconnected}."""
    try:
        p = subprocess.run([SHPOOL, "--no-daemonize", "list"], stdin=subprocess.DEVNULL,
                           stdout=subprocess.PIPE, text=True)
    except OSError as e:
        raise Fail(f"shpool: {e}")
    if p.returncode != 0:
        raise Fail("shpool list failed (is shpool.socket running?)")
    rows = [line.split() for line in p.stdout.splitlines()[1:]]  # after the header
    return {r[0]: r[-1] for r in rows if r}


def check_claudes(d, sessions, idle):
    """The shpool sessions of the Claude sessions in d if all are idle for idle seconds, alone in detached shpool
    sessions and nothing else is using d. Fail says why not."""
    mine = descendants(sessions)
    if os.getpid() in mine:
        raise Fail("this session runs jw")
    if in_use(d, mine):
        raise Fail("in use by other processes")
    for pid, (status, name, at, _) in sessions.items():
        if status != "idle":
            raise Fail(f"Claude {pid} is {status}")
        if not name:
            raise Fail(f"Claude {pid} is not in a shpool session")
        if not isinstance(at, (int, float)) or time.time() - at / 1000 < idle:
            raise Fail(f"Claude {pid} idle for less than {idle}s")
    names = sorted({name for _, name, _, _ in sessions.values()})
    state = shpool_status()
    for n in names:
        if state.get(n) != "disconnected":
            raise Fail(f"shpool session {n} is {state.get(n, 'unknown')}")
    for pid, (_, name, _, _) in sessions.items():
        if not session_top(pid):
            raise Fail(f"shpool session {name} also runs other processes")
    return names


def kill_shpool(d, names):
    """Kill the shpool sessions and wait until nothing is using d."""
    for n in names:
        p = subprocess.run([SHPOOL, "--no-daemonize", "kill", n], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL)
        if p.returncode != 0:
            raise Fail(f"shpool kill {n} failed")
    for _ in range(50):
        if not in_use(d):
            return
        time.sleep(0.1)
    raise Fail(f"still in use after killing shpool session {', '.join(names)}")


def session_top(pid, proc="/proc"):
    """Whether Claude process pid is the first process of its shpool session (parent comm shpool, or parent comm
    event-claude-la whose parent is shpool)."""
    try:
        parent = stat_fields(pid, proc)[1]
        with open(f"{proc}/{parent}/comm") as f:
            comm = f.read().strip()
        if comm == "event-claude-la":  # comm is cut to 15 characters
            parent = stat_fields(parent, proc)[1]
            with open(f"{proc}/{parent}/comm") as f:
                comm = f.read().strip()
        return comm == "shpool"
    except (OSError, IndexError, ValueError):
        return False


def stuck(s):
    # a session waiting on a permission prompt holds its MR's lock, so later events only queue
    return s.get("status") == "idle" or (s.get("status") == "waiting" and s.get("waitingFor") == "permission prompt")


def idle_sessions(claudes, state, now, proc="/proc"):
    """[(shpool session, working directory, Claude session id)] of the detached shpool sessions whose Claudes are all
    idle (or stuck on a permission prompt) for IDLE_LIMIT and the first process of the session, sorted by name."""
    groups = {}
    for pid, s, cwd, name in claudes:
        if name is not None:
            groups.setdefault(name, []).append((pid, s, cwd))
    found = []
    for name, members in sorted(groups.items()):
        if state.get(name) != "disconnected":
            continue
        if all(stuck(s) and isinstance(s.get("statusUpdatedAt"), (int, float))
               and now - s["statusUpdatedAt"] / 1000 >= IDLE_LIMIT and session_top(pid, proc)
               for pid, s, _ in members):
            _, s, cwd = members[0]
            found.append((name, cwd, s.get("sessionId", "?")))
    return found


def agent_workspace(ws):
    """(home, name, path) of the agent workspace a record points to, or None."""
    if not isinstance(ws, str) or "\0" in ws or not os.path.isabs(ws):
        return None
    parent, name = os.path.split(os.path.realpath(ws))
    agents = os.path.basename(parent)
    if os.path.dirname(parent) != os.path.realpath(ROOT) or not re.search(r".\.agents$", agents):
        return None
    repo = agents.removesuffix(".agents")
    if (not os.path.isdir(f"{os.path.dirname(parent)}/{repo}/.jj") or not SESSION_NAME.fullmatch(name)
            or not os.path.isdir(f"{parent}/{name}/.jj")):
        return None
    return f"{ROOT}/{repo}", name, f"{ROOT}/{agents}/{name}"


def load_records(d):
    """[(name, record)] of the session records in d, sorted by name."""
    try:
        files = sorted(os.listdir(d))
    except OSError:
        return []
    found = []
    for f in files:
        name = f.removesuffix(".json")
        record = read_record(d, name) if name != f else None
        if record is not None:
            found.append((name, record))
    return found


def report_stopped(killed, path, seen):
    for name, sid in killed:
        seen.add(name)
        print(f"stopped {name} in {path}: AGENT_SESSION={name} claude --resume {sid}")


def mark_record(name, **fields):
    try:
        update_record(records_dir(), name, fields)
    except (OSError, ValueError, Fail) as e:
        sys.stderr.write(f"jw: {name}: cannot write the session record: {e}\n")


def reap_record(name, r, now, seen):
    """Stop and remove the workspace of the session record r if its work is finished; see cmd_reap."""
    ws = agent_workspace(r.get("workspace"))
    done = r.get("status") == "done"
    if "reaped" in r or ws is None or not (done or isinstance(r.get("mrs"), list) and r["mrs"]):
        return
    home, ws_name, path = ws
    gitlab = bool(GITLAB.match(ws_name))
    removed = f"removed {os.path.basename(home)}.agents/{ws_name}"
    if done:
        try:
            report_stopped(stop_sessions(path, FINISHED_IDLE), path, seen)
        except (Fail, OSError):
            return  # not idle and detached yet
        if not gitlab:
            try:
                report_stopped(retire(home, ws_name, yes=True, idle=FINISHED_IDLE), path, seen)
                print(removed)
            except (Fail, OSError) as e:
                report_stopped(getattr(e, "killed", []), path, seen)
                print(f"kept {path}" + (f": {e}" if str(e) else ""))
        mark_record(name, reaped=now)
        return
    if gitlab:
        return
    if "notified" not in r and claude_sessions(path):
        merged = stack_merged(path)
        if merged is None:
            sys.stderr.write(f"jw: {name}: couldn't check the stack\n")
        elif merged:
            try:
                deliver(name, WRAP_UP, "jw-reap", wait=0)
            except Fail as e:
                sys.stderr.write(f"jw: {name}: {e}\n")
            mark_record(name, notified=now)
            print(f"notified {name}")
        return
    try:
        killed = retire(home, ws_name, yes=True, idle=FINISHED_IDLE)
    except (Fail, OSError) as e:
        report_stopped(getattr(e, "killed", []), path, seen)
        if isinstance(e, (Unchecked, OSError)):  # other Fails: still waiting or not ready
            sys.stderr.write(f"jw: {name}: {e}\n")
        return
    report_stopped(killed, path, seen)
    print(removed)
    mark_record(name, reaped=now)


def cmd_reap(a):
    """Stop the sessions whose work is finished (`jw done`, or their MRs are merged) once they are idle for
    FINISHED_IDLE and detached, and remove their merged workspaces; a session whose MRs got merged is first asked to
    wrap up. Then stop the detached Claude shpool sessions idle for IDLE_LIMIT, whose workspaces stay."""
    with open(f"{runtime_dir()}/jw-reap.lock", "a") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            sys.stderr.write("jw: reap already running\n")
            return
        missing = [t for t in ("jj", "git", "glab", "gh") if not shutil.which(t)]
        if missing:
            sys.stderr.write(f"jw: missing on PATH: {' '.join(missing)}\n")
        seen = set()
        now = time.time()
        records = load_records(records_dir())
        deadline = time.monotonic() + REAP_DEADLINE
        for i, (name, r) in enumerate(records):
            if time.monotonic() > deadline:
                sys.stderr.write(f"jw: reap: out of time, {len(records) - i} record(s) left for the next run\n")
                break
            try:
                reap_record(name, r, now, seen)
            except Exception as e:  # one bad record must not stop the others or the idle pass
                sys.stderr.write(f"jw: {name}: {type(e).__name__}: {e}\n")
        failed = False
        for name, cwd, sid in idle_sessions(live_claudes(), shpool_status(), time.time()):
            if name in seen:
                continue
            try:
                rc = subprocess.run([SHPOOL, "--no-daemonize", "kill", name], stdin=subprocess.DEVNULL,
                                    stdout=subprocess.DEVNULL).returncode
            except OSError:
                rc = 1
            if rc == 0:
                report_stopped([(name, sid)], cwd, seen)
            else:
                sys.stderr.write(f"jw: shpool kill {name} failed\n")
                failed = True
        if failed:
            raise Fail()


def stack_merged(d) -> "bool | None":
    """Run `jj stack status` in d: True if the stack is merged, False if not, None if it couldn't check."""
    try:
        p = subprocess.run(["jj", "stack", "status"], cwd=d, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                           text=True, timeout=300)
    except (OSError, subprocess.TimeoutExpired, UnicodeDecodeError):
        return None
    lines = [l.strip() for l in p.stdout.splitlines() if l.strip()]
    if p.returncode == 1:
        return False
    # an old `jj stack` (a `jj log` alias) exits 0 without the summary line
    return True if p.returncode == 0 and lines and lines[-1] == "merged" else None


def stop_sessions(d, idle) -> list:
    """Stop d's Claude shpool sessions if all are idle for at least idle seconds and detached; return
    [(shpool name, sessionId)]."""
    sessions = claude_sessions(d)
    if not sessions:
        if in_use(d):
            raise Fail("in use by other processes")
        return []
    kill_shpool(d, check_claudes(d, sessions, idle))
    return [(name, sid) for _, name, _, sid in sessions.values()]


def retire(home, name, *, yes, idle) -> list:
    """Remove the workspace name of the repo at home if its stack is merged and nothing would be lost, stopping its
    idle sessions; return the killed sessions. Fail if it can't (Unchecked if it couldn't check)."""
    d = f"{home}.agents/{name}"
    if in_use(d):  # cheap: before the stack check, which can fetch
        sessions = claude_sessions(d)
        if not sessions:
            raise Fail("in use by other processes")
        check_claudes(d, sessions, idle)
    at = ("--ignore-working-copy", "log", "--no-graph", "-r", "@", "-T", "commit_id")
    before = jj(*at, cwd=d)  # before the slow stack check, so a rewrite during it is caught below
    merged = stack_merged(d)
    if merged is False:
        raise Fail("unmerged work")
    if merged is None:
        raise Unchecked("couldn't check the stack")
    asked = [] if yes else ask_lost(d, snapshot=False)  # before the kill: listing needs no snapshot
    killed = stop_sessions(d, idle)
    try:
        ask_lost(d, yes, asked=asked)  # snapshots, and may wait for the user
        if jj(*at, cwd=d) != before:
            raise Fail("changed while removing")
        # visible work that no workspace's or pushed history includes; only logged, the stack is merged
        try:
            heads = jj("--ignore-working-copy", "log", "--no-graph",
                       "-r", "heads(trunk().. ~ ::(working_copies() | remote_bookmarks()))",
                       "-T", 'change_id.shortest() ++ " " ++ description.first_line() ++ "\\n"', cwd=d).splitlines()
        except Fail:
            heads = []
        if heads:
            more = f", ... (+{len(heads) - 10} more)" if len(heads) > 10 else ""
            sys.stderr.write(f"jw: {name}: heads outside the workspaces: {'; '.join(heads[:10])}{more}\n")
        remove(home, name)
    except (Fail, OSError) as e:
        e.killed = killed  # the caller reports the sessions that are gone
        raise
    return killed


def killed_note(killed):
    return f" (killed shpool session {', '.join(sorted({n for n, _ in killed}))})" if killed else ""


def cmd_prune(a):
    """Remove the workspaces whose stack is merged into trunk and that hold nothing else (`jj stack status`), not
    modified within the last hour. Those in use go too if the only users are idle Claude sessions in detached shpool
    sessions, which are killed along with their background jobs. gitlab-context's (gl-*) are left to `gitlab-context gc`, which knows when they are
    reused. Directories jj cannot read (failing to load) are deleted only after asking once for all of them; with
    --yes they are kept, as they may hold unsnapshotted edits."""
    broken_ones = []
    for r in repos():
        home = f"{ROOT}/{r}"
        try:
            names = sorted(os.listdir(f"{home}.agents"))
        except OSError:
            continue
        try:
            known = jj("--ignore-working-copy", "workspace", "list", "-T", 'name ++ "\\n"', cwd=home).split()
        except Fail:
            continue  # jj already said why
        for name in names:
            d = f"{home}.agents/{name}"
            if GITLAB.match(name) or not os.path.isdir(f"{d}/.jj") or recent(d):
                continue
            if not in_use(d) and not loads(d):
                broken_ones.append((r, home, name, known))
                continue
            try:
                killed = retire(home, name, yes=a.yes, idle=3600)
                sys.stderr.write(f"jw: removed {r}.agents/{name}{killed_note(killed)}\n")
            except (Fail, OSError) as e:
                sys.stderr.write(f"jw: kept {r}.agents/{name}" + (f": {e}" if str(e) else "")
                                 + killed_note(getattr(e, "killed", [])) + "\n")
    if shutil.which("gitlab-context"):
        subprocess.run(["gitlab-context", "gc", "--all"], stdout=subprocess.DEVNULL)  # it logs what it removes
    if not broken_ones:
        return
    if a.yes:  # they may hold unsnapshotted edits: only a person deletes them
        for r, _, name, _ in broken_ones:
            sys.stderr.write(f"jw: kept {r}.agents/{name}: jj can't load it; run jw prune interactively to delete\n")
        return
    sys.stderr.write("jw: broken workspaces (jj cannot read them, so it cannot tell what is in them):\n")
    for r, _, name, _ in broken_ones:
        sys.stderr.write(f"  {r}.agents/{name}\n")
    try:
        confirm(f"Delete these {len(broken_ones)}? [y/N] ")
    except Fail as e:
        sys.stderr.write(f"jw: kept them: {e}\n")
        return
    for r, home, name, known in broken_ones:
        remove_broken(home, name, known)
        sys.stderr.write(f"jw: removed {r}.agents/{name}\n")
    for home in {home for _, home, _, _ in broken_ones}:
        rmdir_agents(home)


def agent_env(env):
    """env without the variables that tie a session to the caller's own (repo key, event queue, nested-claude marker).

    CLAUDE_CODE_* from a calling Claude (session id, child marker, messaging socket) would make the new Claude a child
    of the caller: it writes into the caller's transcript and never registers, so jw msg cannot reach it."""
    drop = {"CLAUDECODE", "CLAUDE_PID", "CLAUDE_EFFORT", "AI_AGENT", "CLAUDE_EVENT_SESSION_DIR",
            "KNOWLEDGE_ORIGIN", "CLAUDE_WORKSPACE_BASE", "AGENT_SESSION"}
    return {k: v for k, v in env.items() if k not in drop and not k.startswith("CLAUDE_CODE_")}


def runtime_dir():
    return os.environ.get("XDG_RUNTIME_DIR") or f"/run/user/{os.getuid()}"


def task_record(prompt, workspace):
    record = {"kind": "task", "workspace": workspace}
    prompt = prompt.encode("utf-8", "surrogateescape").decode("utf-8", "replace")
    lines = [l.strip() for l in prompt.splitlines() if l.strip()]
    if lines:
        record["title"] = lines[0][:200]
        record["description"] = prompt[:8000]
    return record


def cmd_agent(a):
    if os.environ.get("KNOWLEDGE_ORIGIN", "").startswith("event:"):  # untrusted comment text must not hand off
        raise Fail("event sessions cannot start agents")
    # argparse keeps the `--` in REMAINDER; after it everything is the prompt, even a first word naming a repo
    args, literal = (a.args[1:], True) if a.args[:1] == ["--"] else (a.args, False)
    first = args[0] if args and not literal else None
    if first in repos():
        home, prompt = f"{ROOT}/{first}", args[1:]
        if prompt[:1] == ["--"]:
            prompt = prompt[1:]
        elif prompt[:1] and prompt[0] in ("-r", "--revision", "-d", "--detach"):
            raise Fail("options go before the repo")
    else:
        if first and args[1:2] == ["--"]:  # `X -- prompt`: X can only be meant as the repo
            raise Fail(f"{first}: not a repo")
        if first and os.path.exists(f"{ROOT}/{first}"):
            raise Fail(f"{first}: not a jj repo")
        prompt = args
        try:
            home = locate()[1]
        except Fail:  # not in a repo: the prompt says which, else pick one
            task = " ".join(prompt)
            tty = sys.stdin.isatty()
            if not tty and not task.strip():
                raise Fail("no repo: pass one (no terminal to pick)")
            guess, why = route(task) if task.strip() else (None, "")
            if guess:
                home = f"{ROOT}/{guess}"
            elif not tty:
                raise Fail(f"no repo chosen ({why})")
            else:
                if why:
                    sys.stderr.write(f"jw: no repo chosen ({why}), pick one\n")
                home = repo_home(None)
    # the default trunk() is resolved in home without a snapshot
    resolve_in = resolve_dir(home) if a.revision else None
    fetch(home)
    # one argument: claude takes a single prompt, and a first word like `update` is no subcommand then
    launch(home, a.detach, ["claude", " ".join(prompt)] if prompt else ["claude"], " ".join(prompt),
           lambda name: add_workspace(home, a.revision or "trunk()", name, resolve_in))


def launch(home, detach, argv, text, make_dir, preferred=None, resume=False):
    """Run argv in a new shpool session of the repo at home, with a record titled after text, and attach to it unless
    detach. The session is called preferred if no shpool session has that name, else s-<time>. make_dir(name) returns the
    directory to run in; it is called under the lock, once the name is chosen. With resume an existing record of the
    name only gets its workspace updated."""
    rundir = runtime_dir()
    with open(f"{rundir}/jw-agent.lock", "w") as lock:  # names are picked and taken (session started) under it
        fcntl.flock(lock, fcntl.LOCK_EX)
        taken = set(shpool_status())
        if preferred and SESSION_NAME.fullmatch(preferred) and preferred not in taken:
            name = preferred
        else:
            base = time.strftime("s-%m%d-%H%M%S")
            name, i = base, 2
            while name in taken or os.path.exists(f"{home}.agents/{name}"):
                name, i = f"{base}-{i}", i + 1
        d = make_dir(name)
        env = agent_env(os.environ) | {"AGENT_SESSION": name}
        fields = {"workspace": d} if resume and read_record(records_dir(), name) is not None else task_record(text, d)
        try:  # the inbox shows the session from its record; a failure must not stop the session
            update_record(records_dir(), name, fields, drop=FINISH_MARKS)  # a (re)started session is not finished
        except (Fail, OSError, ValueError) as e:
            sys.stderr.write(f"jw: cannot write the session record: {e}\n")
        # started from a service (task.new) there is no terminal env, and claude falls back to no colors
        if env.get("TERM", "dumb") == "dumb":
            env["TERM"] = "xterm-256color"
        env.setdefault("COLORTERM", "truecolor")
        # the environment and the prompt go only into this 0600 file: shpool does not pass on the client's
        # environment, and its -c is split like shell words
        fd, job = tempfile.mkstemp(prefix="jw-agent-", dir=rundir)
        with os.fdopen(fd, "w", errors="surrogateescape") as f:
            for k, v in {**env, "SHPOOL_SESSION_NAME": name}.items():
                if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", k):
                    f.write(f"export {k}={shlex.quote(v)}\n")
            f.write('rm -f -- "$0"\n')  # after the exports: PATH is set now
            f.write(f"cd -- {shlex.quote(d)} || exit 1\n")
            f.write(f"exec {DIRENV} exec . {shlex.join(argv)}\n")  # also unloads an inherited direnv env
        try:
            status = subprocess.run([SHPOOL, "--no-daemonize", "attach", "-b", "--ttl", "1d", "-d", d,
                                     "-c", shlex.join([BASH, job]), name]).returncode
        except OSError as e:
            sys.stderr.write(f"jw: shpool: {e}\n")
            status = None
        if status != 0:  # the script removes the file when it starts; the empty workspace is left for `jw prune`
            if os.path.exists(job):
                os.unlink(job)
            raise Fail(f"shpool attach failed for session {name}" if status is not None else "")
    if detach:
        sys.stderr.write(f"jw: started {name} in {os.path.basename(home)}\n")
        sys.stderr.write(f"jw attach {name}\n")  # agent-inbox parses this line (its `N`)
        return
    # if claude has exited, attach would silently create a plain shell session under this name; checking narrows the
    # race, it doesn't close it
    # the daemon can still count the exited `attach -b` client as attached for a moment, and refuses a second one then
    deadline = time.monotonic() + 5
    while (state := shpool_status().get(name)) == "attached" and time.monotonic() < deadline:
        time.sleep(0.1)
    if state is None:
        raise Fail(f"session {name} ended before attaching")
    attach(name)


def project_key(home):
    """The directory name under projects/ that the claude wrapper (__claude_project) gives the repo at home."""
    key = re.sub(r"[^A-Za-z0-9]", "-", home)
    if len(key) > 64:
        crc = subprocess.run([CKSUM], input=home.encode(), stdout=subprocess.PIPE).stdout.split()[0].decode()
        key = f"{key[-50:]}-{crc}"
    return key


def user_text(entry):
    """The text of a transcript's user entry (whitespace collapsed), empty for tool results."""
    message = entry.get("message")
    content = message.get("content") if isinstance(message, dict) else None
    if isinstance(content, list):
        content = " ".join(b["text"] for b in content if isinstance(b, dict) and isinstance(b.get("text"), str))
    return " ".join(content.split()) if isinstance(content, str) else ""


def workspace_root(cwd, home):
    """The root of the workspace of the repo at home that directory cwd is in, by string so that deleted directories
    work; None outside the repo."""
    agents = f"{home}.agents/"
    if cwd.startswith(agents):
        name = cwd[len(agents):].split("/")[0]
        return agents + name if name else None
    if cwd == home or cwd.startswith(home + "/"):
        return home
    return None


def transcript(path, home):
    """(root of the last workspace of the repo at home it worked in, title) of the transcript, None if it has no user
    message or workspace."""
    root = ai = custom = first = None
    with open(path, errors="replace") as f:
        for line in f:
            try:
                e = json.loads(line)
            except ValueError:
                continue
            if not isinstance(e, dict):
                continue
            if isinstance(e.get("cwd"), str):
                root = workspace_root(e["cwd"], home) or root
            if e.get("type") == "ai-title":
                ai = e.get("aiTitle") or ai
            elif e.get("type") == "custom-title":
                custom = e.get("customTitle") or custom
            elif e.get("type") == "user" and not e.get("isMeta"):
                first = first or user_text(e)
    if not (first and root):
        return None
    return root, " ".join(str(custom or ai or first)[:100].split())


def cmd_resume(a):
    if os.environ.get("KNOWLEDGE_ORIGIN", "").startswith("event:"):  # untrusted comment text must not hand off
        raise Fail("event sessions cannot resume sessions")
    home = None
    if not a.repo:
        with contextlib.suppress(Fail):
            home = locate()[1]
    home = home or repo_home(a.repo)
    base = f"{claude_dir()}/projects/{project_key(home)}"
    try:
        files = sorted(((os.path.getmtime(f"{base}/{n}"), n.removesuffix(".jsonl"))
                        for n in os.listdir(base) if n.endswith(".jsonl")), reverse=True)
    except OSError:
        files = []
    live = {s.get("sessionId") for _, s, _, _ in live_claudes()}
    rows = {}
    for mtime, sid in files:
        if sid in live:
            continue
        try:
            found = transcript(f"{base}/{sid}.jsonl", home)
        except OSError:
            continue
        if found and not GITLAB.match(os.path.basename(found[0])):
            when = time.strftime("%m-%d %H:%M", time.localtime(mtime))
            rows[f"{when}\t{os.path.basename(found[0])}\t{found[1]}\t{sid[:8]}"] = (sid, *found)
            if a.last:
                break
    if not rows:
        raise Fail(f"no sessions to resume in {os.path.basename(home)}")
    sid, root, title = rows[next(iter(rows)) if a.last else pick(list(rows))]

    def make_dir(name):
        if os.path.isdir(root):
            return root
        fetch(home)
        d = add_workspace(home, "trunk()", name, None)
        sys.stderr.write(f"jw: {root} is gone, recreated it as {d} on trunk\n")
        return d

    launch(home, a.detach, ["claude", "--resume", sid], title, make_dir, os.path.basename(root), True)


def match_session(names, prefix):
    """The session called prefix, else the only one starting with it."""
    if prefix in names:
        return prefix
    matches = [n for n in names if n.startswith(prefix)]
    if not matches:
        raise Fail(f"no Claude shpool session {prefix}")
    if len(matches) > 1:
        raise Fail(f"{prefix} is ambiguous: {', '.join(sorted(matches))}")
    return matches[0]


def session_rows(found, state, current):
    """fzf lines name, workspace (repo/name, else the directory), shpool state of the sessions in found ({session:
    directory of its Claude}) that shpool has (state); those of the repo `current` first."""
    rows = []
    for name in sorted(found):
        if name not in state:
            continue
        d = found[name]
        while d != "/" and not os.path.isdir(f"{d}/.jj"):
            d = os.path.dirname(d)
        if os.path.isdir(f"{d}/.jj"):
            repo, workspace, _ = layout(d)
            where = f"{repo}/{workspace}"
        else:
            repo, where = None, found[name]
        rows.append((current is None or repo != current, f"{name}\t{where}\t{state[name]}"))
    return [line for _, line in sorted(rows, key=lambda r: r[0])]  # stable


def attach(name):
    try:
        sys.exit(subprocess.run([SHPOOL, "--no-daemonize", "attach", name]).returncode)  # shpool exits with the session's status, e.g. claude's
    except OSError as e:
        raise Fail(f"shpool: {e}")


def cmd_attach(a):
    found = {name: cwd for _, _, cwd, name in live_claudes() if name}
    state = shpool_status()
    if a.name in state:  # also right after `jw agent -d`, before claude has registered its session
        attach(a.name)
        return
    if a.name:
        attach(match_session([n for n in found if n in state], a.name))
        return
    try:
        current = layout(locate()[0])[0]
    except Fail:
        current = None  # not in a repo
    attach(pick(session_rows(found, state, current), empty="no Claude shpool sessions").split("\t")[0])


def cmd_pick(a):
    if not sys.stdin.isatty():
        raise Fail("no terminal to pick a repo")
    try:
        home = repo_home(None)
    except Fail as e:
        if str(e) == "cancelled":
            raise KeyboardInterrupt  # exit 130
        raise
    print(os.path.basename(home))


def sender():
    """Who a message is from: the shpool session, else the workspace, else the user at a terminal."""
    if os.environ.get("SHPOOL_SESSION_NAME"):
        return os.environ["SHPOOL_SESSION_NAME"]
    project = os.environ.get("CLAUDE_PROJECT_DIR", "")
    if re.search(r".\.agents$", os.path.dirname(project)):
        return os.path.basename(project)
    return "user" if "CLAUDECODE" not in os.environ else "unknown"


def message_sessions(name):
    """[(pid, session name, socket path)] of the live Claude sessions called name, working in a directory of that name
    or in that shpool session, that listen for messages."""
    return [(pid, s.get("name") or shpool or "?", s["messagingSocketPath"]) for pid, s, cwd, shpool in live_claudes()
            if isinstance(s.get("messagingSocketPath"), str)
            and name in (s.get("name"), os.path.basename(cwd), shpool)]


def message_content(text, origin):
    """Text as a quote under a header; every line break (also \\r, U+2028/2029, U+0085) starts a quoted line and
    other control characters go, so the text cannot pass for a header."""
    text = re.sub("\r\n|\r|\u2028|\u2029|\u0085", "\n", text).rstrip("\n")
    text = "".join(c for c in text if c in "\t\n" or unicodedata.category(c) != "Cc")
    origin = re.sub(r"[^A-Za-z0-9._-]", "", origin)[:64] or "unknown"
    content = f"Message from {origin}:\n\n" + "\n".join("> " + l for l in text.split("\n"))
    return content if len(content) <= MSG_MAX else content[:MSG_MAX] + "\n[truncated]"


def deliver(name, text, origin, wait=MSG_WAIT):
    """Send text from origin to the running session name, retrying for wait seconds; return the session's label."""
    content = message_content(text, origin)
    line = (json.dumps({"type": "user", "message": {"role": "user", "content": content}}) + "\n").encode()
    deadline = time.monotonic() + wait
    while True:  # a session just started may not have registered or opened its socket yet
        matches = message_sessions(name)
        if len(matches) > 1:
            raise Fail(f"several sessions match {name}: {', '.join(f'{n} (pid {p})' for p, n, _ in matches)}")
        error = Fail(f"no running session {name}")
        if matches:
            _, session, sock = matches[0]
            try:
                with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as s:
                    s.settimeout(5)
                    s.connect(sock)
                    s.sendall(line)
                return session
            except OSError as e:
                error = Fail(f"cannot reach session {name}: {e}")
        if time.monotonic() >= deadline:
            raise error
        time.sleep(0.5)


def cmd_msg(a):
    # event sessions run on untrusted text (GitLab); their messages would become instructions elsewhere
    if os.environ.get("KNOWLEDGE_ORIGIN", "").startswith("event:"):
        raise Fail("event sessions cannot send messages")
    words = a.text[1:] if a.text[:1] == ["--"] else a.text
    text = " ".join(words) if words else sys.stdin.read()
    if not text.strip():
        raise Fail("empty message")
    session = deliver(a.name, text, a.sender or sender(), MSG_WAIT)
    sys.stderr.write(f"jw: sent to {session}\n")


def records_dir(env=None):
    env = os.environ if env is None else env
    state = env.get("XDG_STATE_HOME", "")
    if not os.path.isabs(state):
        state = f"{env.get('HOME') or os.path.expanduser('~')}/.local/state"
    return f"{state}/agent-sessions"


def session_name(name, env=None):
    """name, else the session this process runs in."""
    env = os.environ if env is None else env
    if name is None:
        name = env.get("AGENT_SESSION")
        if not name:
            raise Fail("no session: pass -s NAME or run in an agent session")
    if not SESSION_NAME.fullmatch(name):
        raise Fail(f"invalid session name: {name!r}")
    return name


def read_record(d, name):
    """The record of session name in d, or None if it is missing or not a plain, small JSON object file."""
    if not SESSION_NAME.fullmatch(name):
        return None
    try:
        # O_NONBLOCK: opening a FIFO must not wait for a writer
        fd = os.open(f"{d}/{name}.json", os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(fd, "rb") as f:
            if not stat.S_ISREG(os.fstat(fd).st_mode):
                return None
            data = f.read(RECORD_MAX + 1)
        record = json.loads(data.decode("utf-8")) if len(data) <= RECORD_MAX else None
    except (OSError, ValueError, RecursionError):
        return None
    return record if isinstance(record, dict) else None


def prune_records(d, now):
    """Delete the records in d untouched for RECORD_DAYS; return their file names."""
    try:
        names = sorted(os.listdir(d))
    except OSError:
        return []
    gone = []
    for n in names:
        p = f"{d}/{n}"
        try:
            st = os.lstat(p)
            if n != ".lock" and stat.S_ISREG(st.st_mode) and st.st_mtime < now - RECORD_DAYS * 86400:
                os.unlink(p)
                gone.append(n)
        except OSError:
            pass
    return gone


def locked_update(d, name, change):
    """Under the records lock call change(record) on the record of session name ({} if there is none) and write it
    unless that returns False; return the record."""
    if not SESSION_NAME.fullmatch(name):
        raise Fail(f"invalid session name: {name!r}")
    os.makedirs(d, mode=0o700, exist_ok=True)
    path = f"{d}/{name}.json"
    # polling: a writer that died holding the lock cannot hang the callers (agents, hooks) for good
    with os.fdopen(os.open(f"{d}/.lock", os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600)) as lock:
        deadline = time.monotonic() + LOCK_WAIT
        while True:
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                break
            except BlockingIOError:
                if time.monotonic() >= deadline:
                    raise Fail("session records are busy")
                time.sleep(0.05)
        existed = os.path.lexists(path)
        record = read_record(d, name) or {}
        if change(record) is False:
            return record
        fd, tmp = tempfile.mkstemp(prefix=".tmp-", dir=d)
        try:
            with os.fdopen(fd, "w", encoding="utf-8") as f:
                json.dump(record, f, ensure_ascii=False)
            os.replace(tmp, path)
        except BaseException:
            with contextlib.suppress(FileNotFoundError):
                os.unlink(tmp)
            raise
        if not existed:
            prune_records(d, time.time())
    return record


def update_record(d, name, fields, mrs=(), drop=()):
    """Merge fields (the ones that are not None) and the MR urls into the record of session name, remove the keys in
    drop and return it."""
    def change(record):
        record.update({k: v for k, v in fields.items() if v is not None})
        for k in drop:
            record.pop(k, None)
        if mrs:
            urls = record["mrs"] if isinstance(record.get("mrs"), list) else []
            urls += [u for u in dict.fromkeys(mrs) if u not in urls]
            record["mrs"] = urls
    return locked_update(d, name, change)


def set_status(d, name, state, waiting_for=None, now=None):
    """Record the state of session name; a done session keeps its status, and an unchanged one is not rewritten."""
    def change(record):
        if record.get("status") == "done" or (record.get("status") == state and record.get("waiting_for") == waiting_for):
            return False
        record["status"] = state
        record["status_at"] = now or time.time()
        record.pop("waiting_for", None)
        if waiting_for:
            record["waiting_for"] = waiting_for
    return locked_update(d, name, change)


def cmd_session(a):
    d = records_dir()
    if a.action == "get":
        name = session_name(a.session)
        record = read_record(d, name)
        if record is None:
            raise Fail(f"no session record {name}")
        print(json.dumps(record, ensure_ascii=False))
        return
    # event sessions run on untrusted text (GitLab); the record is shown to the user and its url opened
    if os.environ.get("KNOWLEDGE_ORIGIN", "").startswith("event:"):
        raise Fail("event sessions cannot change session records")
    name = session_name(a.session)
    mrs = a.mr or []
    for url in [a.url, *mrs]:
        if url is not None and (len(url) > URL_MAX or not HTTPS.fullmatch(url)):
            raise Fail(f"not an https url: {url}")
    fields = {"kind": a.kind, "title": a.title, "description": a.description, "url": a.url, "workspace": a.workspace}
    try:
        update_record(d, name, fields, mrs, drop=FINISH_MARKS if a.workspace else ())
    except (OSError, ValueError) as e:  # ValueError: text that is not valid UTF-8
        raise Fail(f"cannot write the session record: {e}")


def agent_session_name():
    name = os.environ.get("AGENT_SESSION")
    if not name:
        raise Fail("not in an agent session")
    if not SESSION_NAME.fullmatch(name):
        raise Fail(f"invalid session name: {name}")
    return name


def cmd_status(a):
    # allowed in event sessions: it only says what the session is doing
    name = agent_session_name()
    if a.waiting_for is not None and a.state != "waiting":
        raise Fail("--waiting-for needs waiting")
    text = "".join(" " if unicodedata.category(c) == "Cc" else c for c in a.waiting_for or "").strip()[:200]
    try:
        set_status(records_dir(), name, a.state, text or None)
    except (OSError, ValueError) as e:
        raise Fail(f"cannot write the session record: {e}")


def cmd_done(a):
    # allowed in event sessions: it only lets the session be stopped
    name = agent_session_name()
    try:
        update_record(records_dir(), name, {"status": "done", "status_at": time.time()}, drop=("waiting_for",))
    except (OSError, ValueError) as e:
        raise Fail(f"cannot write the session record: {e}")
    sys.stderr.write(f"jw: {name} is done; it is stopped once it is idle and detached\n")


def cmd_info(a):
    """Print the workspace and stack of the current directory as JSON (the prompt runs this, so keep it fast)."""
    root = os.getcwd()
    while not os.path.isdir(f"{root}/.jj"):
        if root == "/":
            raise Fail()  # not in a jj repo
        root = os.path.dirname(root)
    repo, workspace, _ = layout(root)
    info = {"repo": repo, "workspace": workspace, "root": root,
            "subdir": os.getcwd()[len(root):].lstrip("/"), "change": None, "stack": None}
    try:
        # same revset as `jj log` in the starship prompt: the stack to trunk() plus @, minus other workspaces' empty working copies
        p = subprocess.run(["jj", "--no-pager", "--ignore-working-copy", "--color=never", "log", "--no-graph",
                            "-r", "(trunk()..(@::) ~ ((working_copies() ~ @) & empty())) | @",
                            "-T", 'if(current_working_copy, change_id.shortest(), "-") ++ '
                                  'if(self.contained_in("trunk()..@"), "\\ty", "\\tn") ++ '
                                  # name=root of every workspace at @ (only the current row), tab separated
                                  'if(current_working_copy, "\\t" ++ working_copies.map(|w| w.name() ++ "=" ++ w.root()).join("\\t")) ++ "\\n"'],
                           cwd=root, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
    except OSError:
        p = None  # no jj
    if p and p.returncode == 0:
        rows = [line.split("\t") for line in p.stdout.splitlines()]
        info["change"] = next((r[0] for r in rows if r[0] != "-"), None)
        info["stack"] = {"index": sum(r[1] == "y" for r in rows), "size": len(rows)}
        mine = [w.partition("=") for r in rows if r[0] != "-" for w in r[2:] if w]
        if len(mine) > 1:  # several workspaces share @: the one whose root is this directory
            mine = [w for w in mine if w[2] and os.path.realpath(w[2]) == os.path.realpath(root)]
        if len(mine) == 1 and mine[0][0]:
            info["workspace"] = mine[0][0]
    print(json.dumps(info))


def parse_args():
    p = argparse.ArgumentParser(prog="jw", description=__doc__.splitlines()[0])
    sub = p.add_subparsers(dest="cmd")
    s = sub.add_parser("new", aliases=["n"], help="create a workspace in a repo and go there")
    s.add_argument("repo", nargs="?", help=f"repo under {ROOT} (default: pick one, the current one first)")
    s.add_argument("-r", "--revision", help="revision, bookmark or bookmark@origin to start from (default: trunk()); when given in this repo, it is resolved in the current workspace")
    s.set_defaults(func=cmd_new)
    s = sub.add_parser("cd", aliases=["c"], help="go to a workspace (default: pick one, this repo's first)")
    s.add_argument("name", nargs="?")
    s.set_defaults(func=cmd_cd)
    s = sub.add_parser("rm", help="forget a workspace and delete its directory (its work stays)")
    s.add_argument("name", nargs="?")
    s.set_defaults(func=cmd_rm)
    s = sub.add_parser("clone", help=f"clone a repo into {ROOT} and go there")
    s.add_argument("url")
    s.add_argument("name", nargs="?", help=f"directory name under {ROOT} (default: from the url)")
    s.set_defaults(func=cmd_clone)
    s = sub.add_parser("prune", help="remove the workspaces whose stack is merged and broken ones in all repos, and the idle detached Claude shpool sessions in merged ones (killing a session kills its background jobs)")
    s.add_argument("--yes", action="store_true", help="don't ask; answer yes to every question")
    s.set_defaults(func=cmd_prune)
    s = sub.add_parser("reap", help="stop the finished sessions (jw done, or merged MRs) once idle and detached, and remove their merged workspaces; stop the detached Claude shpool sessions idle or stuck on a permission prompt for 4 hours (their workspaces stay; resume with claude --resume)")
    s.set_defaults(func=cmd_reap)
    s = sub.add_parser("info", help="print the jj workspace and stack of the current directory as JSON")
    s.set_defaults(func=cmd_info)
    s = sub.add_parser("pick", help="pick a repo (the current one first) and print its name")
    s.set_defaults(func=cmd_pick)
    s = sub.add_parser("agent", aliases=["a"], help="run Claude in a shpool session in a new workspace of a repo (default: this one; outside a repo the one the prompt fits, else pick one)")
    s.add_argument("-d", "--detach", action="store_true", help="start the session in the background")
    s.add_argument("-r", "--revision", help="revision, bookmark or bookmark@origin to start the new workspace from (default: trunk()); when given in this repo, it is resolved in the current workspace")
    s.add_argument("args", nargs=argparse.REMAINDER, help=f"[repo under {ROOT}] [prompt...]; after a leading -- everything is the prompt")
    s.set_defaults(func=cmd_agent)
    s = sub.add_parser("resume", aliases=["r"], help="resume a Claude session of a repo (any of its workspaces; default: this one) in a shpool session, in its workspace (recreated on trunk() if it is gone)")
    s.add_argument("-d", "--detach", action="store_true", help="start the session in the background")
    s.add_argument("-l", "--last", action="store_true", help="resume the newest session without asking")
    s.add_argument("repo", nargs="?", help=f"repo under {ROOT} (default: this one, else pick one)")
    s.set_defaults(func=cmd_resume)
    s = sub.add_parser("attach", aliases=["at"], help="attach to a Claude shpool session (default: pick one, this repo's first); a unique prefix of the name is enough")
    s.add_argument("name", nargs="?")
    s.set_defaults(func=cmd_attach)
    s = sub.add_parser("msg", help="send a message to a running Claude session: delivered right away, between the tool calls of a busy session or as a new turn of an idle one; fails if the session is not running")
    s.add_argument("--from", dest="sender", help="who the message is from (default: this session, else the user)")
    s.add_argument("name", help="the session's name, its workspace name or its shpool session name")
    s.add_argument("text", nargs=argparse.REMAINDER, help="words of the message (default: read stdin); a leading -- is dropped")
    s.set_defaults(func=cmd_msg)
    s = sub.add_parser("session", help="read or update the record of an agent session (kind, title, description, url, workspace, mrs), a JSON file in $XDG_STATE_HOME/agent-sessions that agent-inbox and the notifications show")
    s.set_defaults(func=cmd_session)
    ss = s.add_subparsers(dest="action", required=True)
    s = ss.add_parser("get", help="print the record as JSON; fails if there is none")
    s.add_argument("-s", "--session", help="session name (default: $AGENT_SESSION)")
    s = ss.add_parser("set", help="set the given fields of the record, creating it; --mr adds to the list; refused in GitLab event sessions")
    s.add_argument("-s", "--session", help="session name (default: $AGENT_SESSION)")
    s.add_argument("--kind", help="what started the session, e.g. task, review, MergeRequest")
    s.add_argument("--title")
    s.add_argument("--description")
    s.add_argument("--workspace", help="the session's workspace directory")
    s.add_argument("--url", help="https link to the work item")
    s.add_argument("--mr", action="append", help="https url of a merge request to add (repeatable)")
    s = ss.add_parser("status", help="set the status of this session's record (hooks run this); not for done, see jw done; allowed in GitLab event sessions")
    s.add_argument("state", choices=["busy", "idle", "waiting", "ended"])
    s.add_argument("--waiting-for", help="what the session waits for (with waiting)")
    s.set_defaults(func=cmd_status)
    s = sub.add_parser("done", help="mark this agent session finished: it is stopped once it is idle and detached")
    s.set_defaults(func=cmd_done)
    sub.add_parser("ls", help="list the workspaces")
    return p.parse_args()


def main():
    # info runs on every prompt: skip building the parser
    a = argparse.Namespace(func=cmd_info) if sys.argv[1:] == ["info"] else parse_args()
    try:
        if not hasattr(a, "func"):
            sys.stdout.write(jj("--ignore-working-copy", "workspace", "list"))
            return 0
        a.func(a)
    except Fail as e:
        if str(e):
            print(f"jw: {e}", file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        print("jw: cancelled", file=sys.stderr)
        return 130
    return 0


if __name__ == "__main__":
    sys.exit(main())
