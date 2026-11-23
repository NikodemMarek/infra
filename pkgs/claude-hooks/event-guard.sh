# PreToolUse guard for Edit|Write|Bash: event sessions (AGENT_ORIGIN=event:<type>) don't edit AGENTS.md, CLAUDE.md or the knowledge file ($1).
# (writeShellApplication prepends the shebang and `set -o errexit -o nounset -o pipefail`)
case "${AGENT_ORIGIN:-}" in
event:*) ;;
*) exit 0 ;;
esac

arg="$1"
# shellcheck disable=SC2088 # literal ~/, expanded by hand
case "$arg" in
"~/"*) file="$HOME/${arg#"~/"}" ;;
*) file="$arg" ;;
esac
# shellcheck disable=SC2088
case "$file" in
"$HOME/"*) tilde="~/${file#"$HOME/"}" ;;
*) tilde="$file" ;;
esac

deny() {
	jq -n '{hookSpecificOutput:{hookEventName:"PreToolUse",permissionDecision:"deny",permissionDecisionReason:"Event sessions don'\''t edit AGENTS.md, CLAUDE.md or the knowledge file; report it under **Learnings** in your final message."}}'
	exit 0
}

input="$(cat)"
path="$(jq -r '.tool_input.file_path // empty' <<<"$input")"
cmd="$(jq -r '.tool_input.command // empty' <<<"$input")"

if [ -n "$path" ]; then
	# shellcheck disable=SC2088
	case "$path" in
	/*) ;;
	"~/"*) path="$HOME/${path#"~/"}" ;;
	*) path="$(jq -r '.cwd // empty' <<<"$input")/$path" ;;
	esac
	[ "$(realpath -m "$path")" = "$(realpath -m "$file")" ] && deny
	case "$(basename "$path")" in
	AGENTS.md | CLAUDE.md) deny ;;
	esac
fi

case "$cmd" in
*AGENTS.md* | *CLAUDE.md* | *"$file"* | *"$tilde"* | *"$(basename "$file")"*) deny ;;
esac
exit 0
