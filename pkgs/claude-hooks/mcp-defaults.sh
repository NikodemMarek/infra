# Usage: claude-mcp-defaults DIR
# Starts the project MCP servers of DIR/.mcp.json disabled, so claude doesn't block on its approval dialog:
# every server that is in neither enabledMcpjsonServers nor disabledMcpjsonServers is added to
# disabledMcpjsonServers of DIR/.claude/settings.local.json (`/mcp` enables it later, editing the same file).
# claude merges these keys across settings files and a disabled entry wins, so "decided" is judged over
# $CLAUDE_CONFIG_DIR/settings.json (default ~/.claude), DIR/.claude/settings.json and the local file; only the
# local file is written. Nothing happens without a .mcp.json or with enableAllProjectMcpServers set to true in any
# of them. The file is written (atomically) only when something changes; other keys are kept. Silent on success.
# Unreadable JSON is left alone with a note on stderr. Always exit 0.
dir="${1:-}"
mcp="$dir/.mcp.json"
settings="$dir/.claude/settings.local.json"
if [ -z "$dir" ] || [ ! -f "$mcp" ]; then
  exit 0
fi
files=("${CLAUDE_CONFIG_DIR:-$HOME/.claude}/settings.json" "$dir/.claude/settings.json" "$settings")
for i in "${!files[@]}"; do
  # a missing file reads as no values, like a 0-byte one
  [ -e "${files[$i]}" ] || files[i]=/dev/null
done
# the filter prints nothing when no server needs adding
if ! out="$(jq -n --slurpfile m "$mcp" --slurpfile g "${files[0]}" --slurpfile p "${files[1]}" --slurpfile l "${files[2]}" '
  [($g, $p, $l) | (.[0] // {}) | if type != "object" then error("not an object") else . end] as $all
  | if any($all[]; .enableAllProjectMcpServers == true) then empty else . end
  | ([$all[] | (.enabledMcpjsonServers // []) + (.disabledMcpjsonServers // [])] | add) as $known
  | [($m[0].mcpServers // {}) | keys_unsorted[] | select(. as $n | $known | index($n) | not)] as $add
  | if ($add | length) == 0 then empty
    else $all[2] | .disabledMcpjsonServers = ((.disabledMcpjsonServers // []) + $add) end
  ')"; then
  echo "claude-mcp-defaults: leaving $settings alone" >&2
  exit 0
fi
if [ -z "$out" ]; then
  exit 0
fi
if mkdir -p -- "$dir/.claude" && tmp="$(mktemp -- "$settings.XXXXXX")"; then
  printf '%s\n' "$out" >"$tmp" && mv -f -- "$tmp" "$settings"
  rm -f -- "$tmp"
fi
exit 0
