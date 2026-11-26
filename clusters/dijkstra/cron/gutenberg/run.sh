#!/bin/sh
# sync.sh + symlink.sh under an exclusive lock, so the weekly CronJob and the
# boot-time sync (boot-sync.sh) never run at the same time. After a success the
# kernel boot id (not namespaced, so it is the host's) is stored in
# $STATE_DIR/boot_id; with SKIP_IF_SYNCED=1 nothing is done if it is unchanged.
set -eu
SCRIPTS_DIR="${SCRIPTS_DIR:-/scripts}"
STATE_DIR="${STATE_DIR:-/data/.gutenberg-state}"
BOOT_ID_FILE="${BOOT_ID_FILE:-/proc/sys/kernel/random/boot_id}"
mkdir -p "$STATE_DIR"
exec 9>"$STATE_DIR/lock"
flock 9
if [ "${SKIP_IF_SYNCED:-}" = 1 ] &&
	[ "$(cat "$STATE_DIR/boot_id" 2>/dev/null || true)" = "$(cat "$BOOT_ID_FILE")" ]; then
	echo "already synced since this boot"
	exit 0
fi
"$SCRIPTS_DIR/sync.sh"
"$SCRIPTS_DIR/symlink.sh"
cat "$BOOT_ID_FILE" >"$STATE_DIR/boot_id"
