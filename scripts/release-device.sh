#!/usr/bin/env bash
# Release a previously claimed device via the HTTP lock server.
#
# This script must be SOURCED, not executed.
#
# Usage:
#   . ./scripts/release-device.sh
#
# Expects LOCK_ID and HEARTBEAT_PID to be set by claim-device.sh.
# Also works if the process exits — the server will expire the lock after TTL.
#
# Environment:
#   DEVICE_LOCK_URL — lock server URL (default: http://localhost:7357)

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
    echo "ERROR: this script must be sourced, not executed" >&2
    exit 1
fi

if [[ -n "${LOCK_ID:-}" ]]; then
    local_lock_url="${DEVICE_LOCK_URL:-http://localhost:7357}"
    curl -sf -X DELETE "$local_lock_url/lock/$LOCK_ID" > /dev/null 2>&1 || true
    unset local_lock_url
fi

if [[ -n "${HEARTBEAT_PID:-}" ]]; then
    kill "$HEARTBEAT_PID" 2>/dev/null || true
fi

unset DEVICE_PORT LOCK_ID HEARTBEAT_PID
# Clean up legacy vars if present
unset DEVICE_FD 2>/dev/null || true
