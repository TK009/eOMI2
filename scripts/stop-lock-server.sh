#!/usr/bin/env bash
# Stop the device lock server.
#
# Usage:
#   ./scripts/stop-lock-server.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
PID_FILE="$PROJECT_ROOT/.device-locks/server.pid"

if [[ ! -f "$PID_FILE" ]]; then
    echo "No PID file found — server not running" >&2
    exit 0
fi

pid=$(cat "$PID_FILE")
if kill -0 "$pid" 2>/dev/null; then
    kill "$pid"
    echo "Lock server stopped (PID $pid)"
else
    echo "Lock server not running (stale PID $pid)"
fi
rm -f "$PID_FILE"
