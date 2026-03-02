#!/usr/bin/env bash
# Claim an available ESP32 USB device from the pool.
#
# Usage:
#   eval "$(./scripts/claim-device.sh)"   # sets DEVICE_PORT, DEVICE_LOCK
#   # ... use $DEVICE_PORT ...
#   ./scripts/release-device.sh "$DEVICE_LOCK"
#
# Pass --owner-pid <PID> to record a specific PID as the lock holder
# (useful when the caller is a different process than this script).
#
# The pool is every /dev/ttyUSB* and /dev/ttyACM* present on the system.
# A claimed device has a lockfile at <project-root>/.device-locks/<basename>.lock
# The lockfile contains the PID of the holder.
# Stale lockfiles (holder process died) are automatically cleaned up.
# Lock acquisition is atomic (uses flock to prevent races).

set -euo pipefail

# ── Parse arguments ──────────────────────────────────────────────────────
OWNER_PID=$$
while [[ $# -gt 0 ]]; do
    case "$1" in
        --owner-pid)
            [[ $# -ge 2 ]] || { echo "ERROR: --owner-pid requires a value" >&2; exit 1; }
            [[ "$2" =~ ^[0-9]+$ ]] || { echo "ERROR: --owner-pid must be numeric" >&2; exit 1; }
            OWNER_PID="$2"; shift 2 ;;
        *)           echo "ERROR: unknown argument: $1" >&2; exit 1 ;;
    esac
done

# Always resolve to the main repo root, even from a git worktree.
# git-common-dir points to the main .git; its parent is the project root.
PROJECT_ROOT="$(cd "$(git rev-parse --git-common-dir)/.." && pwd)"
LOCK_DIR="$PROJECT_ROOT/.device-locks"
mkdir -p "$LOCK_DIR"

# Try to atomically claim a device.  flock on a per-pool directory lock
# prevents two concurrent callers from racing on the same device.
exec 9>"$LOCK_DIR/.pool.flock"
flock -x 9

for dev in /dev/ttyUSB* /dev/ttyACM*; do
    [ -e "$dev" ] || continue
    base=$(basename "$dev")
    lock="$LOCK_DIR/${base}.lock"

    if [ -f "$lock" ]; then
        holder=$(cat "$lock" 2>/dev/null || echo "")
        # Lock held by a living process — skip this device
        if [ -n "$holder" ] && kill -0 "$holder" 2>/dev/null; then
            continue
        fi
        # Stale lockfile — remove it
        rm -f "$lock"
    fi

    # Claim: write owner PID
    echo "$OWNER_PID" > "$lock"

    # Release the pool lock before printing output
    flock -u 9

    echo "export DEVICE_PORT='$dev'"
    echo "export DEVICE_LOCK='$lock'"
    exit 0
done

flock -u 9
echo "ERROR: no available ESP32 device found" >&2
exit 1
