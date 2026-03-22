#!/usr/bin/env bash
# Start the device lock server in the background.
#
# Usage:
#   ./scripts/start-lock-server.sh              # default port 7357
#   ./scripts/start-lock-server.sh --port 8080  # custom port
#
# Writes PID to .device-locks/server.pid. No-op if already running.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
PID_DIR="$PROJECT_ROOT/.device-locks"
PID_FILE="$PID_DIR/server.pid"
LOG_FILE="$PID_DIR/server.log"

mkdir -p "$PID_DIR"

# Check if already running
if [[ -f "$PID_FILE" ]]; then
    pid=$(cat "$PID_FILE")
    if kill -0 "$pid" 2>/dev/null; then
        echo "Lock server already running (PID $pid)" >&2
        exit 0
    fi
    rm -f "$PID_FILE"
fi

python3 "$SCRIPT_DIR/device-lock-server.py" "$@" >> "$LOG_FILE" 2>&1 &
echo $! > "$PID_FILE"
echo "Lock server started (PID $!, log: $LOG_FILE)"
