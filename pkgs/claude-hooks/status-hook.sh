# Usage: claude-status-hook busy|idle|waiting|ended
# Keeps the status of the session record (`jw session status`) up to date; only for sessions with a record
# (AGENT_SESSION: started by `jw agent` or an event). `waiting` gets the reason: a question for
# AskUserQuestion, else the permission prompt of the tool. Unless busy, also records when the prompt cache goes
# cold: the start of the last cache write in the transcript (the earliest entry of that response) plus its 5 minute
# or 1 hour TTL.
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
if [ "$state" != busy ]; then
  t=$(jq -r '.transcript_path | strings' <<<"$input" 2>/dev/null)
  if [ -f "$t" ] && [ -r "$t" ]; then
    expires=$(tac "$t" | jq -nRr '
      def ts: try (.timestamp | sub("\\.[0-9]+";"") | fromdateiso8601) catch null;
      last(label $out | foreach (inputs | fromjson? | objects | select(.type=="assistant" and .isSidechain != true)) as $e (
        {};
        ($e | ts) as $t | $e.message.id as $id | $e.message.usage.cache_creation as $c |
        if .found then
          if $id == .id then .ts = ([.ts, $t] | map(select(. != null)) | min) else .stop = true end
        elif ($c|type=="object") and $t != null and (($c.ephemeral_5m_input_tokens//0) + ($c.ephemeral_1h_input_tokens//0) > 0) then
          {found: true, id: $id, ts: $t, ttl: (if ($c.ephemeral_5m_input_tokens//0) > 0 then 300 else 3600 end)}
        else . end;
        if .stop then break $out elif .found then . else empty end
      )) | .ts + .ttl' 2>/dev/null)
    [ -n "$expires" ] && args+=(--cache-expires-at "$expires")
  fi
fi
jw session status "$state" "${args[@]}" >/dev/null 2>&1
exit 0
