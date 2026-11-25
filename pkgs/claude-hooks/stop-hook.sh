# Stop hook. In sessions with a session record (AGENT_SESSION: started by `jw agent`), tells the
# user the session waits for them: sets the record status to idle and posts claude.waiting (agent-inbox only),
# titled with the first line of the last message.
# `key` = session id, so the inbox keeps one item per session (replaced every turn). Never blocks:
# always exit 0.
# always read all of stdin first, so Claude Code never gets EPIPE on a large input
input=$(cat)
[ -n "${AGENT_SESSION:-}" ] || exit 0
claude-status-hook idle <<<"$input"
# a headless worker's turn ends with its result (session.result), not a claude.waiting item
[ -z "${AGENT_HEADLESS:-}" ] || exit 0
jq -c '([.last_assistant_message | strings | split("\n")[] | select(startswith("```") | not)
      | sub("^[#>*\\s-]+"; "") | gsub("\\*\\*|`"; "") | select(. != "")]
    | first // "waiting for you")[0:120] as $title
  | . + {title: $title, timeout: 86400}
  + (if (.session_id | type) == "string" and .session_id != "" then {key: .session_id} else {} end)
  + (if (.last_assistant_message | type) == "string" then {body: .last_assistant_message[0:1000]} else {} end)' <<<"$input" |
	claude-route-event claude.waiting >&2
exit 0
