#!/usr/bin/env bash
# Claim an available ESP32 USB device from the pool.
#
# Usage:
#   eval "$(./scripts/claim-device.sh)"   # sets DEVICE_PORT, DEVICE_LOCK
#   # ... use $DEVICE_PORT ...
#   ./scripts/release-device.sh "$DEVICE_LOCK"
#
# The pool is every /dev/ttyUSB* and /dev/ttyACM* present on the system.
# A claimed device has a lockfile at <project-root>/.device-locks/<basename>.lock
# The lockfile contains the PID of the holder.
# Stale lockfiles (holder process died) are automatically cleaned up.

set -euo pipefail

# Always resolve to the main repo root, even from a git worktree.
# git-common-dir points to the main .git; its parent is the project root.
PROJECT_ROOT="$(cd "$(git rev-parse --git-common-dir)/.." && pwd)"
LOCK_DIR="$PROJECT_ROOT/.device-locks"
mkdir -p "$LOCK_DIR"

find_available_device() {
    for dev in /dev/ttyUSB* /dev/ttyACM*; do
        [ -e "$dev" ] || continue
        local base lock holder
        base=$(basename "$dev")
        lock="$LOCK_DIR/${base}.lock"

        # No lockfile -> available
        if [ ! -f "$lock" ]; then
            echo "$dev" "$lock"
            return 0
        fi

        # Stale lockfile (holder died) -> available
        holder=$(cat "$lock" 2>/dev/null || echo "")
        if [ -n "$holder" ] && ! kill -0 "$holder" 2>/dev/null; then
            rm -f "$lock"
            echo "$dev" "$lock"
            return 0
        fi
    done
    return 1
}

result=$(find_available_device) || {
    echo "echo 'ERROR: no available ESP32 device found'" >&2
    exit 1
}

read -r dev lock <<< "$result"

echo $$ > "$lock"

echo "export DEVICE_PORT='$dev'"
echo "export DEVICE_LOCK='$lock'"
