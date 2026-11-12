# Claude Code statusline: reads the session JSON on stdin, prints one line.
# Best-effort by design: the package enables nounset only (no errexit/pipefail),
# so a missing tool or odd input degrades the line instead of killing it.
input=$(cat)

# Fields from stdin, one per line
{
  read -r dir
  read -r session_name
  read -r start
  read -r model
  read -r ctx
  read -r limits
  read -r cost
  read -r lines
} < <(echo "$input" | jq -r '
  (.workspace.current_dir // .cwd // ""),
  (.session_name // ""),
  (.session.start_time // ""),
  (.model.display_name // ""),
  (.context_window.used_percentage | if . != null then "ctx:\(round)%" else "" end),
  ([(.rate_limits.five_hour.used_percentage | select(. != null) | "5h:\(round)%"),
    (.rate_limits.seven_day.used_percentage | select(. != null) | "7d:\(round)%")] | join(" ")),
  (.cost.total_cost_usd // ""),
  ((.cost.total_lines_added // 0) as $a | (.cost.total_lines_removed // 0) as $r
    | if $a + $r > 0 then "+\($a)/-\($r)" else "" end)')

# Directory
[ -z "$dir" ] && dir=$(pwd)
short_dir="${dir/#"$HOME"/\~}"

# Name the session's jj workspace after the session title (claude-jj-workspace decides; once, in the background)
if [ -n "$session_name" ] && command -v claude-jj-workspace >/dev/null 2>&1; then
  case "$dir" in */*.agents/s-*) printf '%s' "$input" | claude-jj-workspace title >/dev/null 2>&1 & ;; esac
fi

# jj info from `jw info` (never snapshots): project, workspace, @ and its place in the stack.
# Show only the project (plus any subdirectory), not the full path.
jj_info=""
if info=$(cd "$dir" 2>/dev/null && jw info 2>/dev/null); then
  {
    read -r short_dir
    read -r jj_info
  } < <(echo "$info" | jq -r '
    .repo + (if .subdir != "" then "/" + .subdir else "" end),
    "ws:\(.workspace) @\(.change // "")" + (if .stack then " \(.stack.index)/\(.stack.size)" else "" end)')
fi

# Git info (jj repos show jj info instead)
git_branch=""
git_dirty=""
if [ -z "$jj_info" ] && command -v git >/dev/null 2>&1 && git -C "$dir" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  git_branch=$(git -C "$dir" branch --show-current 2>/dev/null)
  if [ -n "$(git -C "$dir" status --porcelain 2>/dev/null)" ]; then
    git_dirty='[$]'
  fi
fi

# jw session: title, kind, status, prompt-cache countdown and MRs (fails without a record)
session=$(jw session get 2>/dev/null | jq -r --arg name "$session_name" '
  ((.title // "") | if . == "" then $name else . end | if length > 40 then .[:39] + "…" else . end) as $title
  | [$title,
     (.kind // "" | if . == "task" or . == "" then "" else "[\(.)]" end),
     (.status // ""),
     (.cache_expires_at | numbers | . - now | if . <= 0 then "cache:cold" else "cache:\(. / 60 | ceil)m" end),
     ((.mrs // [])[]
       | (capture("/merge_requests/(?<n>[0-9]+)") | "!" + .n) // (capture("/pull/(?<n>[0-9]+)") | "#" + .n) // empty)]
  | map(select(. != "")) | join(" ")' 2>/dev/null)
[ -z "$session" ] && session="${session_name:0:40}"

# Cost and lines changed
cost_lines=""
[ -n "$cost" ] && cost_lines=$(printf '$%.2f' "$cost")
[ -n "$lines" ] && cost_lines="${cost_lines:+$cost_lines }$lines"

# Session uptime
if [ -n "$start" ]; then
  now=$(date +%s)
  elapsed=$((now - start / 1000))
  if [ "$elapsed" -ge 3600 ]; then
    uptime="$(( elapsed / 3600 ))h$(( (elapsed % 3600) / 60 ))m"
  elif [ "$elapsed" -ge 60 ]; then
    uptime="$(( elapsed / 60 ))m"
  else
    uptime="${elapsed}s"
  fi
else
  uptime=""
fi

# Build output: starship-like
out="$short_dir"
[ -n "$jj_info" ] && out="$out $jj_info"
[ -n "$git_branch" ] && out="$out on  $git_branch $git_dirty"
[ -n "$session" ] && out="$out | $session"
[ -n "$model" ] && out="$out | $model"
[ -n "$ctx" ] && out="$out | $ctx"
[ -n "$limits" ] && out="$out | $limits"
[ -n "$cost_lines" ] && out="$out | $cost_lines"
[ -n "$uptime" ] && out="$out | ${uptime}"

printf '%s' "$out"
