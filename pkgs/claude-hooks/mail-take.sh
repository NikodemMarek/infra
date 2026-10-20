# Usage: claude-mail-take
# Takes the session messages queued for this session (see the session.message event handler) and prints
# them, separated by a blank line; nothing when there are none. The mailbox is $XDG_RUNTIME_DIR/claude-mail/<name>,
# <name> = basename of CLAUDE_PROJECT_DIR when its parent directory ends in `.agents` (a jw workspace), else
# SHPOOL_SESSION_NAME, else no mailbox. The hook input `cwd` is not used: it follows `cd`. Takes at most 8000
# bytes (additionalContext is capped at 10,000 characters; the rest leaves room for separators and the GitLab
# pointer the hooks add); the rest stays queued for the next turn. Always exit 0.
name=""
if [ -n "${CLAUDE_PROJECT_DIR:-}" ]; then
  case "$(basename -- "$(dirname -- "$CLAUDE_PROJECT_DIR")")" in
    *.agents) name="$(basename -- "$CLAUDE_PROJECT_DIR")" ;;
  esac
fi
if [ -z "$name" ]; then
  name="${SHPOOL_SESSION_NAME:-}"
fi
case "$name" in
  [A-Za-z0-9]*) ;;
  *) exit 0 ;;
esac
case "$name" in
  *[!A-Za-z0-9._-]*) exit 0 ;;
esac
box="${XDG_RUNTIME_DIR:-}/claude-mail/$name"
if [ -z "${XDG_RUNTIME_DIR:-}" ] || [ ! -d "$box/queue" ]; then
  exit 0
fi
messages=""
while IFS= read -r path; do
  messages+="${messages:+$'\n\n'}$(cat -- "$path")"
done < <(claude-event-queue take "$box" 8000)
printf '%s' "$messages"
exit 0
