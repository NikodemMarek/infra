# SUDO_ASKPASS helper: routes the password request to the user via the event router and waits
# for the answer on a reply FIFO (the handler writes the password to it). stdout carries only the password.
# Deliberately no errexit (the package sets only nounset + pipefail): every failure maps to exit 1.
limit=110
reader=""
dir="$(mktemp -d "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/claude-askpass-XXXXXX")" || exit 1
trap 'kill "$reader" 2>/dev/null; rm -rf "$dir"' EXIT
trap 'exit 1' INT TERM HUP
mkfifo -m 600 "$dir/reply" || exit 1

# The parent is sudo; the command line is only informational, so failure is fine.
cmd="$(tr '\0' ' ' 2>/dev/null </proc/$PPID/cmdline)" || cmd=""
cmd="${cmd% }"

jq -n --arg cwd "$PWD" --arg title "${cmd:-sudo}" --arg prompt "${1:-}" --arg cmd "$cmd" --arg reply "$dir/reply" --argjson timeout "$limit" \
  '{cwd: $cwd, title: $title, prompt: $prompt, command: $cmd, reply: $reply, timeout: $timeout}' |
  claude-route-event sudo.password_request >&2 || {
  echo "claude-askpass: could not route the password request" >&2
  exit 1
}

# The FIFO is an argument to a background reader, not a `<` redirect: a redirect would block on open
# before timeout starts. Reading through the `read` builtin (instead of a command substitution) lets
# a trapped signal interrupt the wait at once; the EXIT trap then kills the reader. `exec` makes $!
# the timeout process itself (it forwards TERM to head); --foreground keeps it in our process group.
exec 3< <(exec timeout --foreground "$limit" head -n1 -- "$dir/reply")
reader=$!
IFS= read -r pw <&3
exec 3<&-
if [ -z "$pw" ]; then
  echo "claude-askpass: no password received" >&2
  exit 1
fi
printf '%s\n' "$pw"
