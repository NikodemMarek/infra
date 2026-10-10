# Claude Code statusline: reads the session JSON on stdin, prints one line.
# Best-effort by design: the package enables nounset only (no errexit/pipefail),
# so a missing tool or odd input degrades the line instead of killing it.
input=$(cat)

# Directory
dir=$(echo "$input" | jq -r '.workspace.current_dir // .cwd // empty')
[ -z "$dir" ] && dir=$(pwd)
short_dir="${dir/#"$HOME"/\~}"

# Name the session's jj workspace after the session title (claude-jj-workspace decides; once, in the background)
if [ -n "$(echo "$input" | jq -r '.session_name // empty')" ] && command -v claude-jj-workspace >/dev/null 2>&1; then
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

# Package version (pom.xml or package.json)
pkg_ver=""
if [ -f "$dir/pom.xml" ]; then
  pkg_ver=$(grep -m1 '<version>' "$dir/pom.xml" | sed 's/.*<version>\(.*\)<\/version>.*/\1/' | tr -d ' ')
elif [ -f "$dir/package.json" ]; then
  pkg_ver=$(jq -r '.version // empty' "$dir/package.json" 2>/dev/null)
fi

# Java version
java_ver=""
if command -v java >/dev/null 2>&1; then
  java_ver=$(java -version 2>&1 | head -1 | sed 's/.*"\(.*\)".*/\1/')
fi

# Session uptime
start=$(echo "$input" | jq -r '.session.start_time // empty')
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

# Model + context
model=$(echo "$input" | jq -r '.model.display_name // empty')
used=$(echo "$input" | jq -r '.context_window.used_percentage // empty')
ctx=""
[ -n "$used" ] && ctx=$(printf "ctx:%.0f%%" "$used")

# Build output: starship-like
out="$short_dir"
[ -n "$jj_info" ] && out="$out $jj_info"
[ -n "$git_branch" ] && out="$out on  $git_branch $git_dirty"
[ -n "$pkg_ver" ] && out="$out | v$pkg_ver"
[ -n "$java_ver" ] && out="$out via v$java_ver"
[ -n "$model" ] && out="$out | $model"
[ -n "$ctx" ] && out="$out | $ctx"
[ -n "$uptime" ] && out="$out | ${uptime}"

printf '%s' "$out"
