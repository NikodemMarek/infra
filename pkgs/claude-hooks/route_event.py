#!/usr/bin/env python3
"""Usage: claude-route-event [type]   (stdin: a JSON object)

Posts the object as an event to the event router ($EVENT_ROUTER_URL, default http://127.0.0.1:7531).
Without a type, derives one from a PermissionRequest hook payload: AskUserQuestion ->
claude.question, else claude.permission_request. Adds `project` (last component of cwd), `title`,
`jj` (the output of `jw info` in the cwd) when available, `shpool` (the shpool session name) when run
inside one, and `origin_file` (CLAUDE_ORIGIN_FILE, the session's origin description) when set.

Title (non-empty strings only): .title, else the question, the Bash description / first command line,
"<tool> <file relative to cwd>", "<tool> <url>", the tool name, the type.

Never writes to stdout (Claude parses hook stdout as a decision) and never exits 2 (blocking code):
every failure exits 1 with a message on stderr.
"""

import json
import os
import subprocess
import sys
import urllib.request

DEFAULT_URL = "http://127.0.0.1:7531"


def text(value):
    """The value if it is a string, else ""."""
    return value if isinstance(value, str) else ""


def event_type(arg, inp):
    if arg:
        return arg
    return "claude.question" if inp.get("tool_name") == "AskUserQuestion" else "claude.permission_request"


def title(inp, etype):
    tool_input = inp.get("tool_input")
    if not isinstance(tool_input, dict):
        tool_input = {}
    tool = text(inp.get("tool_name"))
    cwd = text(inp.get("cwd"))
    questions = tool_input.get("questions")
    first = questions[0] if isinstance(questions, list) and questions else None
    question = text(first.get("question")) if isinstance(first, dict) else ""
    bash = text(tool_input.get("description")) or text(tool_input.get("command")).strip().split("\n")[0]
    file_path = text(tool_input.get("file_path"))
    if cwd and file_path.startswith(cwd + "/"):
        file_path = file_path[len(cwd) + 1:]
    url = text(tool_input.get("url"))

    return (text(inp.get("title"))
            or question
            or (bash if tool == "Bash" else "")
            or (tool and file_path and f"{tool} {file_path}")
            or (tool and url and f"{tool} {url}")
            or tool
            or etype)


def jj_info(cwd):
    """Parsed `jw info` of cwd; best effort, None on any failure."""
    try:
        p = subprocess.run(["jw", "info"], cwd=cwd, capture_output=True, text=True,
                           stdin=subprocess.DEVNULL, timeout=2)
        return json.loads(p.stdout) if p.returncode == 0 else None
    except (OSError, subprocess.SubprocessError, ValueError):
        return None


def enrich(inp, etype):
    cwd = text(inp.get("cwd"))
    event = {**inp, "type": etype, "project": cwd.split("/")[-1] or None,"title": title(inp, etype)}
    if os.environ.get("SHPOOL_SESSION_NAME"):
        event["shpool"] = os.environ["SHPOOL_SESSION_NAME"]
    if os.environ.get("CLAUDE_ORIGIN_FILE"):
        event["origin_file"] = os.environ["CLAUDE_ORIGIN_FILE"]
    info = jj_info(cwd) if cwd else None
    if info is not None:
        event["jj"] = info
    return event


def post(event, url):
    req = urllib.request.Request(f"{url}/events", data=json.dumps(event).encode(), method="POST",
                                 headers={"Content-Type": "application/json"})
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))  # never through $http_proxy
    opener.open(req, timeout=2).close()


def main():
    try:
        inp = json.load(sys.stdin)
        if not isinstance(inp, dict):
            raise ValueError("not a JSON object")
    except ValueError:
        print("claude-route-event: invalid input", file=sys.stderr)
        return 1
    event = enrich(inp, event_type(sys.argv[1] if len(sys.argv) > 1 else "", inp))
    url = os.environ.get("EVENT_ROUTER_URL") or DEFAULT_URL
    try:
        post(event, url)
    except Exception:  # connection, timeout, HTTP error status, bad URL
        print(f"claude-route-event: cannot post event to {url}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
