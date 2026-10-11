# Usage: claude-route-event [type]   (stdin: JSON object)
# Posts the object as an event to the event router. Without a type, derives one from a
# PermissionRequest hook payload: AskUserQuestion -> claude.question, else claude.permission_request.
# Never writes to stdout (Claude parses hook stdout as a decision) and never exits 2 (blocking code).
# Deliberately no errexit (the package sets only nounset + pipefail): every failure maps to exit 1.
url="${EVENT_ROUTER_URL:-http://127.0.0.1:7531}"
payload="$(jq -c --arg t "${1:-}" '
  (if $t != "" then $t
   elif .tool_name == "AskUserQuestion" then "claude.question"
   else "claude.permission_request" end) as $type
  | . + {
      type: $type,
      project: ((.cwd // "") | split("/") | last),
      title: (.title // .tool_input.questions[0].question? // .tool_input.command? // .tool_input.file_path? // .tool_name // $type)
    }
')" || {
  echo "claude-route-event: invalid input" >&2
  exit 1
}
curl -q -fsS --noproxy '*' --max-time 2 -o /dev/null -H 'Content-Type: application/json' --data-binary @- "$url/events" <<<"$payload" || {
  echo "claude-route-event: cannot post event to $url" >&2
  exit 1
}
