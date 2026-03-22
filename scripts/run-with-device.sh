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
    if [[ -n "${LOCK_ID:-}" ]]; then
        . "$SCRIPT_DIR/release-device.sh"
    fi
}
trap cleanup EXIT

# Claim device (wait up to 120 s)
# shellcheck disable=SC2034  # consumed by sourced _claim-wait.sh
CLAIM_TIMEOUT=120
. "$SCRIPT_DIR/_claim-wait.sh"
echo "Claimed $DEVICE_PORT" >&2
export DEVICE_PORT

# Expand $DEVICE_PORT in arguments
args=()
for arg in "$@"; do
    args+=("${arg//\$DEVICE_PORT/$DEVICE_PORT}")
done

"${args[@]}"
