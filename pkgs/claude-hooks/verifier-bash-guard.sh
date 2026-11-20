# PreToolUse guard for the verifier's Bash: it may build/test/run, but not modify source,
# history, the system, or anything remote.
# (writeShellApplication prepends the shebang and `set -o errexit -o nounset -o pipefail`)
input="$(cat)"
cmd="$(jq -r '.tool_input.command // empty' <<<"$input")"
deny() {
	jq -n --arg r "$1" '{hookSpecificOutput:{hookEventName:"PreToolUse",permissionDecision:"deny",permissionDecisionReason:$r}}'
	exit 0
}
# Normalise whitespace for matching
c=" $(tr '\n\t' '  ' <<<"$cmd") "

grep -Eq '(^|[;&|( ])sudo |(^|[;&|( ])su ' <<<"$c" && deny "sudo/su not allowed for verifier."
grep -Eq 'git +(-C +[^ ]+ +)?(push|commit|reset|checkout|switch|restore|clean|rebase|merge|cherry-pick|revert|stash|tag|branch +-[dD]|am|apply|rm|mv|add|config)( |$)' <<<"$c" &&
	deny "Mutating git commands are not allowed for verifier (read-only git only)."
grep -Eq '(glab|gh) +[a-z-]+ +(create|merge|close|delete|approve|note|update|run|retry|cancel)' <<<"$c" &&
	deny "Mutating GitLab/GitHub CLI calls are not allowed for verifier."
grep -Eq '(^|[;&|( ])rm +(-[a-zA-Z]*[rRf]|--recursive|--force)' <<<"$c" && deny "rm -r/-f not allowed; use build tool clean targets instead."
grep -Eq '(sed|perl|ruby) +(-[a-zA-Z]*i|--in-place)' <<<"$c" && deny "In-place file edits are not allowed for verifier."
grep -Eq '(curl|wget)[^|]*\| *(ba|z|fi)?sh' <<<"$c" && deny "Piping downloads into a shell is not allowed."
grep -Eq '(^|[;&|( ])(mkfs|dd|shutdown|reboot|chmod|chown|kill|pkill|killall|systemctl|crontab) ' <<<"$c" &&
	deny "System-altering command not allowed for verifier."
grep -Eq '(mvn|gradle|gradlew|npm|pnpm|yarn|cargo) +[^;&|]*(deploy|publish|release)' <<<"$c" &&
	deny "Deploy/publish/release not allowed for verifier."
# Output redirection is only allowed to /tmp, /dev/null or the scratchpad
if grep -Eoq '(^|[^0-9&<>])>>? *[^ &>]' <<<"$c"; then
	while read -r target; do
		case "$target" in /dev/null | /dev/stderr | /dev/stdout | /tmp/* | "&"*) ;; *) deny "Redirecting output to '$target' is not allowed (use /tmp)." ;; esac
	done < <(grep -Eo '>>? *[^ ;&|)]+' <<<"$c" | sed -E 's/^>>? *//')
fi
grep -Eq '(^|[;&|( ])tee +' <<<"$c" && ! grep -Eq 'tee +(-a +)?/tmp/' <<<"$c" && deny "tee only allowed into /tmp."
exit 0
