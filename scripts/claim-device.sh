#!/usr/bin/env bash
# Claim an available ESP32 USB device via the HTTP lock server.
#
# This script must be SOURCED, not executed, so the heartbeat process
# stays alive in the caller's shell.
#
# Usage:
#   . ./scripts/claim-device.sh                                  # auto-select
#   CLAIM_DEVICES="/dev/ttyUSB0" . ./scripts/claim-device.sh     # pin device
#
# On success sets: DEVICE_PORT, LOCK_ID, HEARTBEAT_PID
# On failure returns 1 (all devices locked) or 2 (no devices found).
#
# Release with:  . ./scripts/release-device.sh
# Or just exit — the server will expire the lock after TTL (60 s).
#
# Environment:
#   DEVICE_LOCK_URL — lock server URL (default: http://localhost:7357)
#   CLAIM_DEVICES   — pin to specific device(s)

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    echo "ERROR: this script must be sourced, not executed" >&2
    exit 1
fi

_claim_device() {
    local lock_url="${DEVICE_LOCK_URL:-http://localhost:7357}"

    # Build request body
    local device_arg="any"
    if [[ -n "${CLAIM_DEVICES:-}" ]]; then
        # Use first pinned device (space-separated list — server handles one at a time)
        device_arg="${CLAIM_DEVICES%% *}"
    fi

    local container_id=""
    if [[ -f /proc/1/cpuset ]]; then
        container_id="$(cat /proc/1/cpuset 2>/dev/null || true)"
    elif [[ -f /.dockerenv ]]; then
        container_id="$(hostname 2>/dev/null || true)"
    fi

    local body
    body=$(cat <<EOF
{"device": "$device_arg", "holder": {"pid": $$, "host": "$(hostname 2>/dev/null || echo unknown)", "container": "${container_id:-none}", "time": "$(date -u +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || echo unknown)"}}
EOF
    )

    # POST /lock
    local response http_code
    response=$(curl -sf -w '\n%{http_code}' -X POST \
        -H "Content-Type: application/json" \
        -d "$body" \
        "$lock_url/lock" 2>/dev/null) || {
        echo "ERROR: cannot reach lock server at $lock_url" >&2
        return 1
    }

    http_code=$(echo "$response" | tail -1)
    local json_body
    json_body=$(echo "$response" | sed '$d')

    if [[ "$http_code" != "200" ]]; then
        local err
        err=$(echo "$json_body" | python3 -c "import sys,json; print(json.load(sys.stdin).get('error','unknown'))" 2>/dev/null || echo "$json_body")
        if [[ "$err" == *"no devices"* ]]; then
            echo "ERROR: no USB serial devices found" >&2
            return 2
        fi
        echo "ERROR: $err" >&2
        return 1
    fi

    # Parse response
    local lock_id device_port
    lock_id=$(echo "$json_body" | python3 -c "import sys,json; print(json.load(sys.stdin)['lock_id'])" 2>/dev/null)
    device_port=$(echo "$json_body" | python3 -c "import sys,json; print(json.load(sys.stdin)['device'])" 2>/dev/null)

    if [[ -z "$lock_id" || -z "$device_port" ]]; then
        echo "ERROR: failed to parse lock response" >&2
        return 1
    fi

    # Start background heartbeat (every 30 s, well within 60 s TTL)
    (
        while true; do
            sleep 30
            if ! curl -sf -X POST "$lock_url/lock/$lock_id/heartbeat" > /dev/null 2>&1; then
                break  # lock gone, stop heartbeating
            fi
        done
    ) &
    local hb_pid=$!
    disown "$hb_pid" 2>/dev/null || true

    # shellcheck disable=SC2034  # consumed by caller after sourcing
    DEVICE_PORT="$device_port"
    # shellcheck disable=SC2034
    LOCK_ID="$lock_id"
    # shellcheck disable=SC2034
    HEARTBEAT_PID="$hb_pid"
    return 0
}

_claim_device; __claim_rc=$?
unset -f _claim_device
return $__claim_rc
