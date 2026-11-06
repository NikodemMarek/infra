# PreToolUse guard for Bash: sudo has no TTY here, so it must take the password through SUDO_ASKPASS (-A).
# (writeShellApplication prepends the shebang and `set -o errexit -o nounset -o pipefail`)
cmd="$(jq -r '.tool_input.command // empty')"
c=" $(tr '\n\t' '  ' <<<"$cmd") "
if grep -Eq '(^|[;&|( ])sudo +' <<<"$c" && grep -Eo '(^|[;&|( ])sudo +[^ ]*' <<<"$c" | grep -Eqv 'sudo +-A$'; then
  jq -n '{hookSpecificOutput:{hookEventName:"PreToolUse",permissionDecision:"deny",permissionDecisionReason:"Use `sudo -A <cmd>`: the password prompt goes to the user through SUDO_ASKPASS."}}'
fi
exit 0
