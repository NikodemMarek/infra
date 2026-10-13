# Stop hook for event-started sessions. If the per-MR session dir (CLAUDE_EVENT_SESSION_DIR) has new
# follow-up prompt files, blocks the stop and points Claude at them (paths only, never contents: they hold
# untrusted GitLab comment text); stdout is used only for this decision. Otherwise, in sessions started by
# an event (KNOWLEDGE_ORIGIN=event:*), tells the user the session waits for them: posts claude.waiting
# (notification + agent-inbox). `key` = session id, so the router keeps one item per session (replaced
# every turn). Never blocks otherwise: always exit 0.
# always read all of stdin first, so Claude Code never gets EPIPE on a large input
input=$(cat)
dir="${CLAUDE_EVENT_SESSION_DIR:-}"
if [ -n "$dir" ] && [ -d "$dir" ]; then
  paths="$(claude-event-queue take "$dir")"
  if [ -n "$paths" ]; then
    jq -n --arg r "New GitLab activity on this MR, read: ${paths//$'\n'/ }" '{decision:"block",reason:$r}'
    exit 0
  fi
fi
case "${KNOWLEDGE_ORIGIN:-}" in event:*) ;; *) exit 0 ;; esac
jq -c '. + {title: "waiting for you", timeout: 86400}
  + (if (.session_id | type) == "string" and .session_id != "" then {key: .session_id} else {} end)
  + (if (.last_assistant_message | type) == "string" then {body: .last_assistant_message[0:1000]} else {} end)' <<<"$input" |
  claude-route-event claude.waiting >&2
exit 0
