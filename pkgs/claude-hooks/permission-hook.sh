# PermissionRequest hook. Routes the request to the event router (notification + agent-inbox), but only for
# sessions with an origin (CLAUDE_ORIGIN_FILE: started by `jw agent` or an event); sessions without one
# (hand-started claude) get only the local dialog.
# Waits by default (up to CLAUDE_INBOX_WAIT seconds, default and max 600; 0 = notification only) for an
# answer from agent-inbox on a reply FIFO and returns it as the permission decision. The local dialog is
# shown meanwhile and whichever answer comes first wins; no answer in time = no output = local dialog.
# stdout carries only a decision; never exits 2 (blocking code).
# Deliberately no errexit (the package sets only nounset + pipefail): every failure maps to exit 0.
[ -n "${CLAUDE_ORIGIN_FILE:-}" ] || { cat >/dev/null; exit 0; }
w="${CLAUDE_INBOX_WAIT:-600}"
if ! [[ "$w" =~ ^[1-9][0-9]{0,5}$ ]]; then
  exec claude-route-event
fi
if [ "$w" -gt 600 ]; then
  w=600
fi

input="$(cat)"
reader=""
dir="$(mktemp -d "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/claude-hook-XXXXXX")" || {
  printf '%s' "$input" | claude-route-event >&2
  exit 0
}
trap 'kill "$reader" 2>/dev/null; rm -rf "$dir"' EXIT
trap 'exit 1' INT TERM HUP
# Without a FIFO there is nothing to wait on: route the notification only.
mkfifo -m 600 "$dir/reply" || {
  printf '%s' "$input" | claude-route-event >&2
  exit 0
}

# Reader first, then post: the answer can arrive as soon as the event is queued. The FIFO is an argument
# to a background reader (not a `<` redirect, which would block on open before timeout starts); see askpass.sh.
exec 3< <(exec timeout --foreground "$w" head -n1 -- "$dir/reply")
reader=$!
printf '%s' "$input" |
  jq -c --arg reply "$dir/reply" --argjson timeout "$w" '. + {reply: $reply, timeout: $timeout}' |
  claude-route-event >&2 || exit 0

IFS= read -r line <&3
exec 3<&-
if [ -z "$line" ]; then
  exit 0
fi

# Only the decision comes from the reply; the tool input is Claude's own. The one exception is
# AskUserQuestion, where the answers (keyed by the question texts) are added to its input.
# The input goes through stdin (printf is a builtin), never argv: tool_input can exceed the arg limit.
printf '%s\n%s\n' "$input" "$line" | jq -cn '
  input as $in | input as $r
  | if ($r | type) != "object" then empty
    elif $r.behavior == "deny" then {behavior: "deny", message: "Denied by the user from agent-inbox."}
    elif $r.behavior != "allow" then empty
    elif $in.tool_name != "AskUserQuestion" then (if $r | has("answers") then empty else {behavior: "allow"} end)
    elif ($r.answers | type) == "object" and ($r.answers | length) > 0
         and ([$r.answers[]] | all(type == "string" and . != ""))
         and ([$r.answers | keys[]] - [$in.tool_input.questions[]?.question] == [])
      then {behavior: "allow", updatedInput: ($in.tool_input + {answers: $r.answers})}
    else empty end
  | {hookSpecificOutput: {hookEventName: "PermissionRequest", decision: .}}
' 2>/dev/null
exit 0
