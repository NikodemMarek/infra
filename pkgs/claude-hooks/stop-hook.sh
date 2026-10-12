# Stop hook: in sessions started by an event (KNOWLEDGE_ORIGIN=event:*) tells the user the session
# waits for them: posts claude.waiting (notification + agent-inbox). `key` = session id, so the router
# keeps one item per session (replaced every turn). Never writes stdout, never blocks: always exit 0.
case "${KNOWLEDGE_ORIGIN:-}" in event:*) ;; *) exit 0 ;; esac
jq -c '. + {title: "waiting for you", timeout: 86400}
  + (if (.session_id | type) == "string" and .session_id != "" then {key: .session_id} else {} end)
  + (if (.last_assistant_message | type) == "string" then {body: .last_assistant_message[0:1000]} else {} end)' |
  claude-route-event claude.waiting >&2
exit 0
