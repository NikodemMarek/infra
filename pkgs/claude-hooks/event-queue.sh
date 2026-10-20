# Usage: claude-event-queue take DIR [MAX_BYTES]
# Moves the follow-up prompt files DIR/queue/*.md to DIR/read/ (in sorted order) and prints the new
# absolute paths, one per line; nothing when the queue is empty or missing. A file that cannot be moved
# was claimed by another consumer and is skipped. With MAX_BYTES, files are moved while the total size
# moved stays within it: the first file always moves, then it stops at the first file that would exceed
# MAX_BYTES, and that file and the rest stay queued. Other usage: message on stderr, exit 2.
max="${3:-}"
if [ "$#" -lt 2 ] || [ "$#" -gt 3 ] || [ "$1" != take ]; then
  echo "usage: claude-event-queue take DIR [MAX_BYTES]" >&2
  exit 2
fi
if [ "$#" -eq 3 ]; then
  case "$max" in
    '' | *[!0-9]*)
      echo "usage: claude-event-queue take DIR [MAX_BYTES]" >&2
      exit 2
      ;;
  esac
fi
mkdir -p "$2/read"
dir="$(realpath "$2")"
total=0
moved=0
for f in "$dir"/queue/*.md; do
  [ -e "$f" ] || continue
  if [ -n "$max" ]; then
    size="$(stat -c %s "$f")" || continue
    if [ "$moved" -gt 0 ] && [ $((total + size)) -gt "$max" ]; then
      break
    fi
  fi
  mv -T "$f" "$dir/read/${f##*/}" 2>/dev/null || continue
  moved=$((moved + 1))
  total=$((total + ${size:-0}))
  echo "$dir/read/${f##*/}"
done
exit 0
