{
  writeShellApplication,
  coreutils,
  util-linux,
  jq,
  jw,
  forward-notify,
}:
# event handler: a session.message {to, text} event becomes `jw msg` to that session, started with
# `jw resume` if it is not running. The sender is fixed; a `from` in the event is ignored.
# Event data is untrusted: `to` must be a plain session name. A failure is forwarded as a notification.
writeShellApplication {
  name = "event-handler-session-message";
  runtimeInputs = [coreutils util-linux jq jw forward-notify];
  text = ''
    event=$(cat)
    session=""

    fail() {
      echo "session-message: not delivered: $1" >&2
      jq -cn --arg session "$session" --arg reason "$1" '
        (if $session != "" then {session: $session} else {} end) + {title: "not delivered: \($reason)"}
      ' | ER_TYPE=session.message event-handler-forward-notify || true
      exit 1
    }

    reason() {
      local last=''${1##*$'\n'}
      echo "''${last#jw: }"
    }

    deliver() {
      printf '%s' "$text" | jw msg --from events "$to"
    }

    to=$(jq -r '(.to | strings) // ""' <<<"$event" 2>/dev/null) || to=""
    text=$(jq -r '(.text | strings) // ""' <<<"$event" 2>/dev/null) || text=""

    name_re='^[A-Za-z0-9][A-Za-z0-9._-]*$'
    [[ $to =~ $name_re ]] || fail "invalid session name"
    session=$to
    [[ $text =~ [^[:space:]] ]] || fail "empty text"

    if err=$(deliver 2>&1); then
      exit 0
    fi
    [[ $err == *"no running session"* ]] || fail "$(reason "$err")"

    # one resume per session at a time; whoever waited on the lock tries again before resuming
    exec 9>"$XDG_RUNTIME_DIR/session-message-$to.lock"
    flock -w 300 9 || fail "busy"
    if err=$(deliver 2>&1); then
      exit 0
    fi
    [[ $err == *"no running session"* ]] || fail "$(reason "$err")"

    # 9>&-: the resumed session must not inherit the lock
    if ! err=$(timeout 5m jw resume -d --session "$to" </dev/null 9>&- 2>&1 >/dev/null); then
      [[ $err == *"is running"* ]] || fail "cannot resume: $(reason "$err")"
    fi
    for attempt in 1 2 3; do
      if err=$(deliver 2>&1); then
        exit 0
      fi
      ((attempt == 3)) || sleep 10
    done
    fail "$(reason "$err")"
  '';
}
