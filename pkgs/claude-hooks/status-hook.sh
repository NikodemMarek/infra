# Usage: claude-status-hook busy|idle|waiting|ended
# Keeps the status of the session record (`jw session status`) up to date; only for sessions with a record
# (AGENT_SESSION: started by `jw agent` or an event). `waiting` gets the reason: a question for
# AskUserQuestion, else the permission prompt of the tool.
# Prints nothing (SessionStart and UserPromptSubmit stdout becomes model context); always exit 0.
# Deliberately no errexit (the package sets only nounset + pipefail): every failure maps to exit 0.
# always read all of stdin first, so Claude Code never gets EPIPE on a large input
input=$(cat)
[ -n "${AGENT_SESSION:-}" ] || exit 0
state="${1:-}"
args=()
if [ "$state" = waiting ]; then
  tool=$(jq -r '.tool_name | strings' <<<"$input" 2>/dev/null)
  if [ "$tool" = AskUserQuestion ]; then
    args=(--waiting-for question)
  elif [ -n "$tool" ]; then
    args=(--waiting-for "permission: $tool")
  else
    args=(--waiting-for permission)
  fi
fi
jw session status "$state" "${args[@]}" >/dev/null 2>&1
exit 0
