#!/usr/bin/env bash
# Wait for a device to become available, retrying claim-device.sh.
#
# This script must be SOURCED. Expects SCRIPT_DIR to be set.
#
# Optional env vars:
#   CLAIM_TIMEOUT   — max wait in seconds (default: 120)
#   CLAIM_INTERVAL  — retry interval in seconds (default: 5)
#
# On success: DEVICE_PORT and DEVICE_FD are set.
# On failure: returns 1.

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    echo "ERROR: this script must be sourced, not executed" >&2
    exit 1
fi

_claim_wait() {
    local timeout="${CLAIM_TIMEOUT:-120}"
    local interval="${CLAIM_INTERVAL:-5}"
    local waited=0 rc

    while true; do
        rc=0
        . "$SCRIPT_DIR/claim-device.sh" 2>/dev/null || rc=$?

        [[ $rc -eq 0 ]] && return 0

        # No devices exist at all — fail fast, don't retry
        if [[ $rc -eq 2 ]]; then
            echo "ERROR: no USB serial devices found" >&2
            return 1
        fi

        if [[ $waited -ge $timeout ]]; then
            echo "ERROR: timed out waiting for a device after ${timeout}s" >&2
            return 1
        fi

        echo "All devices busy (waited ${waited}s/${timeout}s). Current holders:" >&2
        local lock_dir
        lock_dir="$(cd "$(git rev-parse --git-common-dir)/.." && pwd)/.device-locks"
        local lf
        for lf in "$lock_dir"/*.lock; do
            [[ -f "$lf" ]] || continue
            echo "  $(basename "$lf"): $(tr '\n' ' ' < "$lf" 2>/dev/null)" >&2
        done
        sleep "$interval"
        waited=$((waited + interval))
    done
}

_claim_wait; __cwait_rc=$?
unset -f _claim_wait
return $__cwait_rc
