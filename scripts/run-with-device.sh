#!/usr/bin/env bash
# Run a command with an automatically claimed ESP32 device.
#
# Usage:
#   ./scripts/run-with-device.sh espflash flash --port '$DEVICE_PORT' fw.bin
#   ./scripts/run-with-device.sh bash              # interactive shell
#   CLAIM_DEVICES="/dev/ttyUSB0" ./scripts/run-with-device.sh minicom -D '$DEVICE_PORT'
#
# The device is released automatically when the command exits.
# Literal '$DEVICE_PORT' in arguments is expanded to the claimed port.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [[ $# -eq 0 ]]; then
    echo "Usage: $0 <command> [args...]" >&2
    echo "  Literal '\$DEVICE_PORT' in args is replaced with the claimed port." >&2
    exit 1
fi

# Release device on exit
cleanup() {
    if [[ -n "${DEVICE_FD:-}" ]]; then
        . "$SCRIPT_DIR/release-device.sh"
    fi
}
trap cleanup EXIT

# Claim with wait loop (120 s)
CLAIM_TIMEOUT=120
CLAIM_INTERVAL=5
WAITED=0

while true; do
    if . "$SCRIPT_DIR/claim-device.sh" 2>/dev/null; then
        break
    fi

    if [[ $WAITED -ge $CLAIM_TIMEOUT ]]; then
        echo "ERROR: timed out waiting for a device after ${CLAIM_TIMEOUT}s" >&2
        exit 1
    fi

    echo "All devices busy (waited ${WAITED}s/${CLAIM_TIMEOUT}s), retrying..." >&2
    sleep "$CLAIM_INTERVAL"
    WAITED=$((WAITED + CLAIM_INTERVAL))
done

echo "Claimed $DEVICE_PORT" >&2
export DEVICE_PORT

# Expand $DEVICE_PORT in arguments
args=()
for arg in "$@"; do
    args+=("${arg//\$DEVICE_PORT/$DEVICE_PORT}")
done

"${args[@]}"
