{
  writeShellApplication,
  coreutils,
  jq,
  curl,
  jw,
}:

# event handler: a task.new event starts an agent session with `jw agent -d`
# and reports task.started, or task.failed with what is needed to retry.
# Event data is untrusted: image paths are kept only if they are regular files
# (no symlinks) under $XDG_RUNTIME_DIR/agent-task/, where the capture tool puts
# them, so an event cannot make the session read arbitrary files.
writeShellApplication {
  name = "event-handler-task";
  runtimeInputs = [
    coreutils
    jq
    curl
  ];
  text = ''
    event=$(cat)
    # after the PATH: a jw already there (e.g. with another projectsRoot) wins
    export PATH="$PATH:${jw}/bin"

    repo=$(jq -r '(.repo | strings) // ""' <<<"$event")
    text=$(jq -r '((.prompt | strings) // "") | gsub("^\\s+|\\s+$"; "")' <<<"$event")

    allowed=$(realpath -m -- "''${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/agent-task")
    images=()
    while IFS= read -r p; do
      [[ "$p" == /* && -f "$p" && ! -L "$p" ]] || continue
      real=$(realpath -e -- "$p") || continue
      if [[ "$real" == "$allowed"/* ]]; then
        images+=("$real")
      fi
    done < <(jq -r '(.images // []) | if type == "array" then .[] | strings else empty end' <<<"$event")

    prompt=$text
    if ((''${#images[@]} > 0)); then
      block="Attached images (open each with the Read tool):"
      for p in "''${images[@]}"; do
        block+=$'\n'"- $p"
      done
      prompt=''${text:+$text$'\n\n'}$block
    fi

    title=$(jq -r '((.prompt | strings) // "") | gsub("^\\s+|\\s+$"; "") | split("\n") | map(select(test("\\S"))) | (.[0] // "") | .[0:200]' <<<"$event")
    if [[ -z "$title" ]]; then
      if ((''${#images[@]} > 0)); then
        title="(images only)"
      else
        title="(no task)"
      fi
    fi

    post() {
      local body=$1
      if ! curl -fsS --noproxy '*' -m 5 -H 'Content-Type: application/json' --data-binary @- \
        "''${EVENT_ROUTER_URL:-http://127.0.0.1:7531}/events" <<<"$body" >/dev/null; then
        echo "event-handler-task: cannot post to the event router: $body" >&2
        exit 1
      fi
    }

    failed() {
      local body
      # id, repo and prompt come from the event on stdin: argv strings are capped at 128 KiB
      body=$(jq -c --arg title "$title" --arg reason "$1" '
        ((.id | strings) // "") as $id
        | ((.repo | strings) // "") as $repo
        | {type: "task.failed"}
        + (if $id != "" then {id: $id, key: $id} else {} end)
        + {project: (if $repo != "" then $repo else "task" end), title: $title, reason: $reason, prompt: ((.prompt | strings) // ""), images: $ARGS.positional}
        + (if $repo != "" then {repo: $repo} else {} end)
        + {timeout: 86400}
      ' --args "''${images[@]}" <<<"$event")
      post "$body"
      exit 0
    }

    if [[ -z "$text" && ''${#images[@]} == 0 && -z "$repo" ]]; then
      failed "empty task"
    fi

    # repo goes to jw as an argument: it must not look like an option
    if [[ -n "$repo" && ! "$repo" =~ ^[A-Za-z0-9_.][A-Za-z0-9_.-]*$ ]]; then
      failed "invalid repo"
    fi

    # jw gets the prompt as one argv string, capped at 128 KiB
    if (($(printf %s "$prompt" | wc -c) > 100000)); then
      failed "task too long"
    fi

    args=(agent -d)
    if [[ -n "$repo" ]]; then
      args+=("$repo" --)
    elif [[ -n "$prompt" ]]; then
      args+=(--)
    fi
    if [[ -n "$prompt" ]]; then
      args+=("$prompt")
    fi

    rc=0
    err=$(jw "''${args[@]}" </dev/null 2>&1 >/dev/null) || rc=$?

    session=""
    started_repo=""
    last=""
    while IFS= read -r line; do
      if [[ "$line" =~ ^jw:\ started\ ([^\ ]+)\ in\ (.+)$ ]]; then
        session=''${BASH_REMATCH[1]}
        started_repo=''${BASH_REMATCH[2]}
      fi
      if [[ "$line" =~ [^[:space:]] ]]; then
        last=$line
      fi
    done <<<"$err"

    if [[ "$rc" != 0 || -z "$session" ]]; then
      reason=''${last#jw: }
      failed "''${reason:-jw exited with $rc}"
    fi

    body=$(jq -c --arg project "$started_repo" --arg title "$title" --arg shpool "$session" '
      ((.id | strings) // "") as $id
      | {type: "task.started"}
      + (if $id != "" then {id: $id} else {} end)
      + {project: $project, title: $title, shpool: $shpool}
    ' <<<"$event")
    post "$body"
  '';
}
