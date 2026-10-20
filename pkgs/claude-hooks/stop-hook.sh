# Stop hook. If the per-MR session dir of an event-started session (CLAUDE_EVENT_SESSION_DIR) has new
# follow-up prompt files, or session messages are queued for this session (claude-mail-take), blocks the
# stop: the reason is a pointer to the files (paths only, never contents: they hold untrusted GitLab comment
# text), then the messages inline; stdout is used only for this decision. Otherwise, in sessions with an
# origin (CLAUDE_ORIGIN_FILE: started by `jw agent` or an event), tells the user the session waits for them:
# posts claude.waiting (notification + agent-inbox), titled with the first line of the last message.
# `key` = session id, so the router keeps one item per session (replaced every turn). Never blocks
# otherwise: always exit 0.
# always read all of stdin first, so Claude Code never gets EPIPE on a large input
input=$(cat)
pointer=""
dir="${CLAUDE_EVENT_SESSION_DIR:-}"
if [ -n "$dir" ] && [ -d "$dir" ]; then
  paths="$(claude-event-queue take "$dir")"
  if [ -n "$paths" ]; then
    pointer="New GitLab activity on this MR, read: ${paths//$'\n'/ }"
  fi
fi
messages="$(claude-mail-take)"
if [ -n "$pointer" ] || [ -n "$messages" ]; then
  reason="$pointer"
  if [ -n "$messages" ]; then
    reason+="${pointer:+$'\n\n'}$messages"
  fi
  jq -n --arg r "$reason" '{decision:"block",reason:$r}'
  exit 0
fi
[ -n "${CLAUDE_ORIGIN_FILE:-}" ] || exit 0
jq -c '([.last_assistant_message | strings | split("\n")[] | select(startswith("```") | not)
      | sub("^[#>*\\s-]+"; "") | gsub("\\*\\*|`"; "") | select(. != "")]
    | first // "waiting for you")[0:120] as $title
  | . + {title: $title, timeout: 86400}
  + (if (.session_id | type) == "string" and .session_id != "" then {key: .session_id} else {} end)
  + (if (.last_assistant_message | type) == "string" then {body: .last_assistant_message[0:1000]} else {} end)' <<<"$input" |
  claude-route-event claude.waiting >&2
exit 0
