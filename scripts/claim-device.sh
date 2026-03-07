#!/usr/bin/env bash
# Claim an available ESP32 USB device using flock-based locking.
#
# This script must be SOURCED, not executed, so the flock fd stays open
# in the caller's process.
#
# Usage:
#   . ./scripts/claim-device.sh                                  # auto-select
#   CLAIM_DEVICES="/dev/ttyUSB0" . ./scripts/claim-device.sh     # pin device
#
# On success sets: DEVICE_PORT, DEVICE_FD
# On failure returns 1 (all devices locked) or 2 (no devices found).
#
# Release with:  . ./scripts/release-device.sh
# Or just exit — the kernel releases flocks when the process dies.

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    echo "ERROR: this script must be sourced, not executed" >&2
    exit 1
fi

_claim_device() {
    # Lock directory: override with DEVICE_LOCK_DIR, otherwise derive from
    # the git common-dir.  A shared override is essential when the rig has
    # multiple independent clones (crew) alongside worktrees (polecats) that
    # would otherwise resolve to different directories.
    local lock_dir
    if [[ -n "${DEVICE_LOCK_DIR:-}" ]]; then
        lock_dir="$DEVICE_LOCK_DIR"
    else
        local project_root
        project_root="$(cd "$(git rev-parse --git-common-dir)/.." && pwd)"
        lock_dir="$project_root/.device-locks"
    fi
    mkdir -p "$lock_dir"

    # Build device list
    local devices=()
    if [[ -n "${CLAIM_DEVICES:-}" ]]; then
        # shellcheck disable=SC2086  # intentional word splitting on space-separated paths
        local d
        for d in $CLAIM_DEVICES; do
            if [[ -e "$d" ]]; then
                devices+=("$d")
            else
                echo "WARNING: pinned device $d does not exist, skipping" >&2
            fi
        done
    else
        local g
        for g in /dev/ttyUSB* /dev/ttyACM*; do
            [[ -e "$g" ]] && devices+=("$g")
        done
    fi

    if [[ ${#devices[@]} -eq 0 ]]; then
        echo "ERROR: no USB serial devices found" >&2
        return 2
    fi

    # Fisher-Yates shuffle (no shuf dependency)
    local i j tmp
    for (( i=${#devices[@]}-1; i>0; i-- )); do
        j=$(( RANDOM % (i+1) ))
        tmp="${devices[$i]}"
        devices[i]="${devices[j]}"
        devices[j]="$tmp"
    done

    # Try to flock each device
    local dev base lockfile fd
    for dev in "${devices[@]}"; do
        base="${dev##*/}"
        lockfile="$lock_dir/${base}.lock"

        # Open read-write without truncating — prevents destroying debug info
        # written by the current lock holder when a competitor opens the file.
        exec {fd}<>"$lockfile" || {
            echo "ERROR: cannot open $lockfile" >&2
            return 1
        }

        if flock -n "$fd"; then
            # Got the lock — write debug info (best-effort)
            local container_id=""
            if [[ -f /proc/1/cpuset ]]; then
                container_id="$(cat /proc/1/cpuset 2>/dev/null || true)"
            elif [[ -f /.dockerenv ]]; then
                container_id="$(hostname 2>/dev/null || true)"
            fi
            cat >"$lockfile" <<EOF
PID=$$
HOST=$(hostname 2>/dev/null || echo unknown)
CONTAINER=${container_id:-none}
TIME=$(date -u +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || echo unknown)
EOF
            # shellcheck disable=SC2034  # consumed by caller after sourcing
            DEVICE_PORT="$dev"
            # shellcheck disable=SC2034  # consumed by caller after sourcing
            DEVICE_FD="$fd"
            return 0
        fi

        # Lock held by someone else — close fd and try next
        exec {fd}>&-
    done

    echo "ERROR: all devices are currently locked" >&2
    return 1
}

_claim_device; __claim_rc=$?
unset -f _claim_device
return $__claim_rc
