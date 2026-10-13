# UserPromptSubmit hook: in event-started sessions (CLAUDE_EVENT_SESSION_DIR is a directory) hands Claude
# the paths of new follow-up prompt files (never their contents: they hold untrusted GitLab comment text)
# as additional context. stdout carries only that context; never exits 2 (blocking code).
cat >/dev/null
dir="${CLAUDE_EVENT_SESSION_DIR:-}"
if [ -z "$dir" ] || [ ! -d "$dir" ]; then
  exit 0
fi
paths="$(claude-event-queue take "$dir")"
if [ -n "$paths" ]; then
  jq -n --arg c "New GitLab activity on this MR, read: ${paths//$'\n'/ }" \
    '{hookSpecificOutput:{hookEventName:"UserPromptSubmit",additionalContext:$c}}'
fi
exit 0
