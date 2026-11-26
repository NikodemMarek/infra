#!/bin/sh
# Mirrors the epubs into $DEST_DIR; symlink.sh finds what changed by ctime.
set -eu
DEST_DIR="${DEST_DIR:-/data/gutenberg}"
mkdir -p "$DEST_DIR"
rc=0
rsync -a --stats --timeout 600 --delete --prune-empty-dirs \
	--include='*/' \
	--include='*images-3.epub' \
	--exclude='*' \
	aleph.gutenberg.org::gutenberg-epub "$DEST_DIR" || rc=$?
# 24: source files vanished during the transfer, harmless
if [ "$rc" -eq 24 ]; then
	rc=0
fi
exit "$rc"
