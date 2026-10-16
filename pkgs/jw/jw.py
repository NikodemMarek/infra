#!/usr/bin/env python3
"""jw: quick jj workspaces for the repos under one root (~/projects unless the package sets another), at <repo>.agents/<name> (the layout the claude wrapper uses too).

new, cd, rm and clone print the directory to go to on stdout; the `jw` fish function cds there.
Everything else goes to stderr.
"""

import argparse
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import time

ROOT = os.path.expanduser("~/projects")
FZF = "fzf"  # the nix package pins the store paths
SHPOOL = "shpool"
BASH = "bash"
KNOWLEDGE = "knowledge"  # the nix package pins it
NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*$")
GITLAB = re.compile(r"^gl-[a-z]+-[0-9]+(-[0-9]+)?$")  # gitlab-context's workspaces: it reuses and cleans them


class Fail(Exception):
    pass


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
        raise Fail("cancelled")  # agent-inbox matches this line (its `N`)
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
    """Guess the repo for the task with `knowledge route`; None = no confident guess."""
    names = repos()
    if not names:
        return None
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
        sys.stderr.write(f"jw: could not choose a repo ({why}), pick one\n")
        return None
    try:
        answer = json.loads(p.stdout)
        repo, reason = answer["repo"], answer.get("reason", "")
    except (ValueError, TypeError, KeyError):  # not JSON, not an object, or no repo key
        repo, reason = None, "bad answer"
    if repo not in names:
        sys.stderr.write(f"jw: no repo fits clearly ({reason}), pick one\n")
        return None
    sys.stderr.write(f"jw: {repo}: {reason}\n")
    return repo


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


def cmd_new(a):
    home = repo_home(a.repo)
    try:
        jj("--ignore-working-copy", "git", "fetch", cwd=home)
    except Fail:
        sys.stderr.write("jw: fetch failed, continuing\n")
    name = time.strftime("w-%m%d-%H%M%S")
    # resolve first: `workspace add` leaves its directory behind on a bad revision
    commits = jj("--ignore-working-copy", "log", "--no-graph", "-r", a.revision,
                 "-T", 'if(root, "root", commit_id) ++ "\\n"', cwd=home).split()
    if len(commits) != 1:
        raise Fail(f"{a.revision!r} must be exactly one commit, not {len(commits)}")
    if commits[0] == "root":
        raise Fail(f"{a.revision} resolves to the root commit; pass -r")
    d = f"{home}.agents/{name}"
    os.makedirs(os.path.dirname(d), exist_ok=True)
    jj("workspace", "add", "--name", name, "-r", commits[0], d, cwd=home)
    print(d)


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
    if not NAME.match(name) or name.endswith(".agents"):
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


def confirm(question):
    """Ask on the terminal; Fail unless the answer is y."""
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


def ask_lost(d):
    """Snapshot the workspace at d and, if files jj does not track would be lost with it, ask whether to delete."""
    jj("status", cwd=d)  # snapshot: its work stays in its change after the workspace is gone
    lost = untracked(d)
    if lost:
        more = f", ... (+{len(lost) - 5} more)" if len(lost) > 5 else ""
        confirm(f"jw: {len(lost)} file(s) in {d} are not tracked by jj (ignored or too large) "
                f"and will be lost: {', '.join(lost[:5])}{more}. Delete? [y/N] ")


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
    if name == "default" or not NAME.match(name) or not os.path.isdir(f"{d}/.jj"):
        raise Fail(f"no workspace at {d}")
    inside = (os.path.realpath(os.getcwd()) + "/").startswith(os.path.realpath(d) + "/")
    ask_lost(d)
    remove(home, name)
    if inside:
        print(home)


def stat_fields(pid):
    """Fields 3.. of /proc/<pid>/stat (the command name, field 2, may contain spaces and parentheses): ppid is
    [1], the start time (field 22) [19]."""
    with open(f"/proc/{pid}/stat") as f:
        return f.read().rpartition(")")[2].split()


def claude_sessions(d):
    """{pid: (status, shpool session or None, statusUpdatedAt in ms or None)} of the live Claude sessions with their
    working directory in d."""
    d = os.path.realpath(d)
    base = f"{os.environ.get('CLAUDE_CONFIG_DIR') or os.path.expanduser('~/.local/share/claude')}/sessions"
    try:
        files = os.listdir(base)
    except OSError:
        return {}
    found = {}
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
        if cwd == d or cwd.startswith(d + "/"):
            prefix = b"SHPOOL_SESSION_NAME="
            name = next((e[len(prefix):].decode(errors="replace") for e in env if e.startswith(prefix)), None)
            found[pid] = (s.get("status"), name, s.get("statusUpdatedAt"))
    return found


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


def check_claudes(d, sessions):
    """The shpool sessions of the Claude sessions in d if all are idle for an hour, alone in detached shpool sessions
    and nothing else is using d. Fail says why not."""
    mine = descendants(sessions)
    if os.getpid() in mine:
        raise Fail("this session runs jw")
    if in_use(d, mine):
        raise Fail("in use by other processes")
    for pid, (status, name, at) in sessions.items():
        if status != "idle":
            raise Fail(f"Claude {pid} is {status}")
        if not name:
            raise Fail(f"Claude {pid} is not in a shpool session")
        if not isinstance(at, (int, float)) or time.time() - at / 1000 < 3600:
            raise Fail(f"Claude {pid} idle for less than an hour")
    names = sorted({name for _, name, _ in sessions.values()})
    state = shpool_status()
    for n in names:
        if state.get(n) != "disconnected":
            raise Fail(f"shpool session {n} is {state.get(n, 'unknown')}")
    for pid, (_, name, _) in sessions.items():  # the shpool daemon is the parent of the session's first process
        with open(f"/proc/{stat_fields(pid)[1]}/comm") as f:
            if f.read().strip() != "shpool":
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


def removable(d):
    """Whether the workspace at d holds nothing. Not --ignore-working-copy: snapshot first, so unsnapshotted edits
    count. Merged: @ is in trunk (snapshotted edits make a new commit that is not); after a squash-merge `jj sync`
    leaves an empty @."""
    out = jj("log", "--no-graph", "-T", "change_id", "-r",
             '@ & (::trunk() | (empty() & description(exact:"") & visible_heads()))', cwd=d)
    return bool(out.strip())


def cmd_prune(a):
    """Remove the workspaces that hold nothing: empty and undescribed (a visible head) or merged into trunk, not
    modified within the last hour. Those in use go too if the only users are idle Claude sessions in detached shpool
    sessions, which are killed along with their background jobs. gitlab-context's (gl-*) are left to `gitlab-context gc`, which knows when they are
    reused. Directories jj cannot read (failing to load) are deleted only after asking once for all of them."""
    broken_ones = []
    for r in repos():
        home = f"{ROOT}/{r}"
        try:
            names = sorted(os.listdir(f"{home}.agents"))
        except OSError:
            continue
        try:
            jj("--ignore-working-copy", "git", "fetch", cwd=home)  # to see what has been merged
        except Fail:
            sys.stderr.write(f"jw: {r}: fetch failed, continuing\n")
        try:
            known = jj("--ignore-working-copy", "workspace", "list", "-T", 'name ++ "\\n"', cwd=home).split()
        except Fail:
            continue  # jj already said why
        for name in names:
            d = f"{home}.agents/{name}"
            if GITLAB.match(name) or not os.path.isdir(f"{d}/.jj") or recent(d):
                continue
            busy = in_use(d)
            sessions = claude_sessions(d) if busy else {}
            if busy and not sessions:
                continue  # something else is using it
            try:
                if busy:
                    try:
                        killable = check_claudes(d, sessions)
                    except (Fail, OSError):
                        continue  # not ours to kill; no jj call for that
                if not loads(d):
                    if not busy:
                        broken_ones.append((r, home, name, known))
                    continue
                if not removable(d):
                    continue
                if busy:
                    sys.stderr.write(f"jw: {r}.agents/{name}: will kill shpool session(s) {', '.join(killable)}\n")
                ask_lost(d)  # may wait for the user: check again afterwards
                if busy:
                    killable = check_claudes(d, claude_sessions(d))
                    kill_shpool(d, killable)
                    if not removable(d):
                        raise Fail("changed while pruning")
                remove(home, name)
                sys.stderr.write(f"jw: removed {r}.agents/{name}"
                                 + (f" (killed shpool session {', '.join(killable)})" if busy else "") + "\n")
            except (Fail, OSError) as e:
                sys.stderr.write(f"jw: kept {r}.agents/{name}" + (f": {e}" if str(e) else "") + "\n")
    if shutil.which("gitlab-context"):
        subprocess.run(["gitlab-context", "gc", "--all"], stdout=subprocess.DEVNULL)  # it logs what it removes
    if not broken_ones:
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


def session_name(base):
    """base, or base-2, base-3, ...: the first not taken by a shpool session."""
    taken = set(shpool_status())
    name, i = base, 2
    while name in taken:
        name, i = f"{base}-{i}", i + 1
    return name


def cmd_agent(a):
    # argparse keeps the `--` in REMAINDER; after it everything is the prompt, even a first word naming a repo
    args, literal = (a.args[1:], True) if a.args[:1] == ["--"] else (a.args, False)
    repo = args[0] if args and not literal and os.path.isdir(f"{ROOT}/{args[0]}/.jj") else None
    prompt = args[1:] if repo else args
    if repo:
        workdir = f"{ROOT}/{repo}"
    else:
        try:  # the claude wrapper gives a session of its own to the main checkout and the w-* workspaces
            root, home = locate()
            workdir = root if root == home or layout(root)[1].startswith("w-") else home
        except Fail:  # not in a repo: the prompt says which, else pick one
            guess = route(" ".join(prompt)) if prompt else None
            workdir = f"{ROOT}/{guess}" if guess else repo_home(None)
    name = session_name(layout(workdir)[0])
    # the environment and the prompt go only into this 0600 file: shpool does not pass on the client's
    # environment, and its -c is split like shell words
    rundir = os.environ.get("XDG_RUNTIME_DIR") or f"/run/user/{os.getuid()}"
    fd, job = tempfile.mkstemp(prefix="jw-agent-", dir=rundir)
    with os.fdopen(fd, "w", errors="surrogateescape") as f:
        for k, v in os.environ.items():
            if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", k):
                f.write(f"export {k}={shlex.quote(v)}\n")
        f.write(f"export SHPOOL_SESSION_NAME={shlex.quote(name)}\n")
        f.write('rm -f -- "$0"\n')  # after the exports: PATH is set now
        f.write(f"cd -- {shlex.quote(workdir)} || exit 1\n")
        # one argument: claude takes a single prompt, and a first word like `update` is no subcommand then
        f.write(f"exec {shlex.join(['claude', ' '.join(prompt)] if prompt else ['claude'])}\n")
    cmd = [SHPOOL, "--no-daemonize", "attach", "--ttl", "1d", "-d", workdir,
           "-c", shlex.join([BASH, job]), name]
    if a.detach:
        cmd.insert(3, "-b")
    try:
        status = subprocess.run(cmd).returncode
    except OSError as e:
        status = None
        sys.stderr.write(f"jw: shpool: {e}\n")
    # the script removes it when it starts; a detached one may not have started yet
    if (status != 0 or not a.detach) and os.path.exists(job):
        os.unlink(job)
    if status is None:
        raise Fail("")
    if not a.detach:
        sys.exit(status)  # attached: shpool exits with the session's status, e.g. claude's
    if status != 0:
        raise Fail(f"shpool attach failed for session {name}")
    sys.stderr.write(f"shpool attach {name}\n")  # agent-inbox parses this line (its `N`)


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
    s.add_argument("-r", "--revision", default="trunk()", help="start from (default: trunk())")
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
    s = sub.add_parser("prune", help="remove empty, merged and broken workspaces in all repos, and the idle detached Claude shpool sessions in merged ones (killing a session kills its background jobs)")
    s.set_defaults(func=cmd_prune)
    s = sub.add_parser("info", help="print the jj workspace and stack of the current directory as JSON")
    s.set_defaults(func=cmd_info)
    s = sub.add_parser("agent", aliases=["a"], help="run Claude in a shpool session in a repo (default: this one; outside a repo the one the prompt fits, else pick one)")
    s.add_argument("-d", "--detach", action="store_true", help="start the session in the background")
    s.add_argument("args", nargs=argparse.REMAINDER, help=f"[repo under {ROOT}] [prompt...]")
    s.set_defaults(func=cmd_agent)
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
