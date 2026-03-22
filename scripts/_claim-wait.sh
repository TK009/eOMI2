#!/usr/bin/env bash
# Wait for a device to become available, retrying claim-device.sh.
#
# This script must be SOURCED. Expects SCRIPT_DIR to be set.
#
# Optional env vars:
#   CLAIM_TIMEOUT   — max wait in seconds (default: 120)
#   CLAIM_INTERVAL  — retry interval in seconds (default: 5)
#
# On success: DEVICE_PORT, LOCK_ID, and HEARTBEAT_PID are set.
# On failure: returns 1.

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    echo "ERROR: this script must be sourced, not executed" >&2
    exit 1
fi

_claim_wait() {
    local timeout="${CLAIM_TIMEOUT:-120}"
    local interval="${CLAIM_INTERVAL:-5}"
    local lock_url="${DEVICE_LOCK_URL:-http://localhost:7357}"
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

        # Show current holders from the server
        echo "All devices busy (waited ${waited}s/${timeout}s). Current holders:" >&2
        curl -sf "$lock_url/devices" 2>/dev/null | python3 -c "
import sys, json
try:
    data = json.load(sys.stdin)
    for d in data.get('devices', []):
        if d['status'] == 'locked':
            h = d.get('holder', {})
            info = ' '.join(f'{k}={v}' for k, v in h.items())
            dev = d['device'].split('/')[-1]
            print(f'  {dev}.lock: {info}', file=sys.stderr)
except: pass
" || true

        sleep "$interval"
        waited=$((waited + interval))
    done
}

_claim_wait; __cwait_rc=$?
unset -f _claim_wait
return $__cwait_rc
