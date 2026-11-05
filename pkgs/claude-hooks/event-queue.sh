# Usage: claude-event-queue take DIR
# Moves the follow-up prompt files DIR/queue/*.md to DIR/read/ (in sorted order) and prints the new
# absolute paths, one per line; nothing when the queue is empty or missing. A file that cannot be moved
# was claimed by another consumer and is skipped. Other usage: message on stderr, exit 2.
if [ "$#" -ne 2 ] || [ "$1" != take ]; then
  echo "usage: claude-event-queue take DIR" >&2
  exit 2
fi
mkdir -p "$2/read"
dir="$(realpath "$2")"
for f in "$dir"/queue/*.md; do
  [ -e "$f" ] || continue
  mv -T "$f" "$dir/read/${f##*/}" 2>/dev/null || continue
  echo "$dir/read/${f##*/}"
done
exit 0
