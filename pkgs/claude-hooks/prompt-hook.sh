# UserPromptSubmit hook: hands Claude, as additional context, the paths of new follow-up prompt files (in
# event-started sessions: CLAUDE_EVENT_SESSION_DIR is a directory; never their contents: they hold untrusted
# GitLab comment text) and the session messages queued for this session (claude-mail-take), inline.
# stdout carries only that context; never exits 2 (blocking code).
cat >/dev/null
context=""
dir="${CLAUDE_EVENT_SESSION_DIR:-}"
if [ -n "$dir" ] && [ -d "$dir" ]; then
  paths="$(claude-event-queue take "$dir")"
  if [ -n "$paths" ]; then
    context="New GitLab activity on this MR, read: ${paths//$'\n'/ }"
  fi
fi
messages="$(claude-mail-take)"
if [ -n "$messages" ]; then
  context+="${context:+$'\n\n'}$messages"
fi
if [ -n "$context" ]; then
  jq -n --arg c "$context" \
    '{hookSpecificOutput:{hookEventName:"UserPromptSubmit",additionalContext:$c}}'
fi
exit 0
