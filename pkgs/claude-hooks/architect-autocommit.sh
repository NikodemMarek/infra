# PostToolUse hook for the architect: auto-commit the edited file in the project memory repo.
# Deliberately no errexit (the package sets only nounset + pipefail): a failing git call must never fail the hook.
MEM="${CLAUDE_MEMORY_DIR:-}"
if [ -z "$MEM" ]; then
	jq -n '{systemMessage: "memory: CLAUDE_MEMORY_DIR is not set; the edit was not committed"}'
	exit 0
fi
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
# Serialise concurrent architect commits.
lockdir="${XDG_RUNTIME_DIR:-/tmp/user-$(id -u)}/claude-memory"
mkdir -p "$lockdir" 2>/dev/null
(
	flock -w 30 9 || exit 0
	git -c core.hooksPath=/dev/null add -A -- "$rel" >/dev/null 2>&1
	git -c core.hooksPath=/dev/null diff --cached --quiet -- "$rel" >/dev/null 2>&1 && exit 0
	git -c core.hooksPath=/dev/null commit -q --no-verify -m "architect: update $rel" -- "$rel" >/dev/null 2>&1 || true
) 9>"$lockdir/memory.lock"
# Files below a nested repository can never be committed by the memory repo; say so.
nested="$(find "$real" -mindepth 2 -name .git -print -quit 2>/dev/null)"
if [ -n "$nested" ]; then
	jq -n --arg p "$nested" '{systemMessage: ("memory: nested git repository at " + $p + "; files below it are not committed")}'
fi
exit 0
