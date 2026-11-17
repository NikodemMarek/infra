# PostToolUse hook for the architect: auto-commit the edited file in the project memory repo.
# Deliberately no errexit (the package sets only nounset + pipefail): a failing git call must never fail the hook.
MEM="${CLAUDE_MEMORY_DIR:-${CLAUDE_CONFIG_DIR:-$HOME/.claude}/memory}"
input="$(cat)"
path="$(jq -r '.tool_input.file_path // empty' <<<"$input")"
real="$(realpath "$MEM" 2>/dev/null)" || exit 0
[ -n "$path" ] || exit 0
# Resolve before the cd below, so a relative path means relative to the hook's cwd.
abs="$(realpath -m -- "$path" 2>/dev/null)" || exit 0
# Never let git walk up into an enclosing repo (e.g. a dotfiles repo containing the data dir).
GIT_CEILING_DIRECTORIES="$(dirname "$real")"
export GIT_CEILING_DIRECTORIES
cd "$real" || exit 0
top="$(git rev-parse --show-toplevel 2>/dev/null)" || exit 0
[ "$top" = "$real" ] || exit 0
# Only the edited file is committed, and only if it lives inside the memory repo.
case "$abs" in
"$real"/?*) ;;
*) exit 0 ;;
esac
rel="${abs#"$real"/}"
# Serialise with other memory writers (the knowledge tooling takes the same lock).
lockdir="${XDG_RUNTIME_DIR:-/tmp/user-$(id -u)}/knowledge"
mkdir -p "$lockdir" 2>/dev/null
commit_err="$(
  (
    flock -w 30 9 || exit 0
    git -c core.hooksPath=/dev/null add -A -- "$rel" >/dev/null 2>&1
    git -c core.hooksPath=/dev/null diff --cached --quiet -- "$rel" >/dev/null 2>&1 && exit 0
    out="$(git -c core.hooksPath=/dev/null commit -q --no-verify -m "architect: update $rel" -- "$rel" 2>&1 >/dev/null)" \
      || printf '%s\n' "${out%%$'\n'*}"
  ) 9>"$lockdir/memory.lock"
)"
msg=""
[ -z "$commit_err" ] || msg="memory: commit of $rel failed: $commit_err"
# Files below a nested repository can never be committed by the memory repo; say so.
nested="$(find "$real" -mindepth 2 -name .git -print -quit 2>/dev/null)"
if [ -n "$nested" ]; then
  msg="${msg:+$msg; }memory: nested git repository at $nested; files below it are not committed"
fi
# One JSON object at most: the hook's stdout must parse as a single document.
[ -z "$msg" ] || jq -n --arg m "$msg" '{systemMessage: $m}'
exit 0
