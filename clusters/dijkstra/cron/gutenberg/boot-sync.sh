#!/bin/sh
# Runs run.sh once per boot of the node (run.sh skips the sync if the boot id
# is already recorded). A failure is logged and not retried until the next boot
# (or pod restart), then the pod just idles.
set -eu
SCRIPTS_DIR="${SCRIPTS_DIR:-/scripts}"
# give the node time to settle (network, other pods) after boot
sleep "${BOOT_DELAY:-900}"
if ! SKIP_IF_SYNCED=1 "$SCRIPTS_DIR/run.sh"; then
	echo "boot sync failed, not retrying until the next boot" >&2
fi
exec sleep infinity
