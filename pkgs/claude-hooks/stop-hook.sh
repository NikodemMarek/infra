# Stop hook. In sessions with a session record (AGENT_SESSION: started by `jw agent`), sets the record
# status to idle and posts claude.waiting with the last message as body, which jw relays to the supervisor.
# Never blocks and never writes to stdout: always exit 0.
# always read all of stdin first, so Claude Code never gets EPIPE on a large input
input=$(cat)
[ -n "${AGENT_SESSION:-}" ] || exit 0
claude-status-hook idle <<<"$input"
# a headless worker's turn ends with its result (session.result), not a claude.waiting event
[ -z "${AGENT_HEADLESS:-}" ] || exit 0
jq -c --arg session "$AGENT_SESSION" '{session: $session}
  + (if (.last_assistant_message | type) == "string" then {body: .last_assistant_message} else {} end)' <<<"$input" |
	event-pub claude.waiting >&2
exit 0
