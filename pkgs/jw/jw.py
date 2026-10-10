#!/usr/bin/env python3
"""jw: quick jj workspaces for the repos under one root (~/projects unless the package sets another), at <repo>.agents/<name> (the layout the claude wrapper uses too).

new, cd, rm and clone print the directory to go to on stdout; the `jw` fish function cds there.
Everything else goes to stderr.
"""

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import time

ROOT = os.path.expanduser("~/projects")
FZF = "fzf"  # the nix package pins the store path
NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*$")
AGENT = re.compile(r"^(agent-[0-9a-f]+|s-[0-9]+-[0-9]+-[0-9]+)$")  # the workspaces of Claude subagents and sessions


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
    if p.returncode != 0:
        raise Fail()  # cancelled
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


def in_use(d):
    """Whether some process has its working directory in d."""
    d = os.path.realpath(d)
    for pid in os.listdir("/proc"):
        if not pid.isdigit():
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


def remove(home, name):
    d = f"{home}.agents/{name}"
    jj("status", cwd=d)  # snapshot: its work stays in its change after the workspace is gone
    lost = untracked(d)
    if lost:
        more = f", ... (+{len(lost) - 5} more)" if len(lost) > 5 else ""
        confirm(f"jw: {len(lost)} file(s) in {d} are not tracked by jj (ignored or too large) "
                f"and will be lost: {', '.join(lost[:5])}{more}. Delete? [y/N] ")
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
    remove(home, name)
    if inside:
        print(home)


def cmd_prune(a):
    """Remove the session (w-*) and Claude (agent-*, s-*) workspaces that hold nothing: empty, undescribed, a visible
    head, not in use. Claude workspaces modified within the last hour stay. Directories jj cannot read
    (failing to load) are deleted only after asking once for all of them."""
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
            agent = AGENT.match(name)
            if not (name.startswith("w-") or agent) or not os.path.isdir(f"{d}/.jj") or in_use(d):
                continue
            try:
                broken = not loads(d)
                if (broken or agent) and recent(d):
                    continue
                if broken:
                    broken_ones.append((r, home, name, known))
                    continue
                # not --ignore-working-copy: snapshot first, so unsnapshotted edits count
                empty = jj("log", "--no-graph", "-T", "change_id", "-r",
                           '@ & empty() & description(exact:"") & visible_heads()', cwd=d)
                if not empty.strip():
                    continue
                remove(home, name)
                sys.stderr.write(f"jw: removed {r}.agents/{name}\n")
            except Fail as e:
                sys.stderr.write(f"jw: kept {r}.agents/{name}" + (f": {e}" if str(e) else "") + "\n")
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
    s = sub.add_parser("prune", help="remove empty and broken workspaces in all repos")
    s.set_defaults(func=cmd_prune)
    s = sub.add_parser("info", help="print the jj workspace and stack of the current directory as JSON")
    s.set_defaults(func=cmd_info)
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
    return 0


if __name__ == "__main__":
    sys.exit(main())
