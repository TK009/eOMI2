#!/usr/bin/env bash
# Orchestrate an end-to-end test run against a real ESP32 device.
#
# Steps:
#   1. Claim a USB device from the pool
#   2. Build firmware (unless --skip-build)
#   3. Flash the device and capture serial output
#   4. Wait for the Wi-Fi IP address in serial output (30 s timeout)
#   5. Health-check the HTTP server (15 s timeout)
#   6. Run the pytest e2e suite
#   7. Clean up (release device, kill monitor, remove temp files)
#
# Usage:
#   ./scripts/run-e2e.sh                   # full run
#   ./scripts/run-e2e.sh --skip-build      # reuse existing firmware
#   ./scripts/run-e2e.sh -- -k test_boot   # pass extra args to pytest

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# In a git worktree, target/ and .env live in the main repo root.
# git-common-dir points to the main .git; its parent is the repo root.
REPO_ROOT="$(cd "$(git -C "$PROJECT_ROOT" rev-parse --git-common-dir)/.." && pwd)"

SERIAL_LOG=$(mktemp /tmp/e2e-serial.XXXXXX)
MONITOR_PID=""
DEVICE_LOCK=""
SKIP_BUILD=false
PYTEST_ARGS=()

# ── Parse arguments ──────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
    case "$1" in
        --skip-build) SKIP_BUILD=true; shift ;;
        --)          shift; PYTEST_ARGS=("$@"); break ;;
        *)           PYTEST_ARGS+=("$1"); shift ;;
    esac
done

# ── Cleanup trap ─────────────────────────────────────────────────────────
cleanup() {
    echo "── Cleaning up ──"
    if [[ -n "$MONITOR_PID" ]] && kill -0 "$MONITOR_PID" 2>/dev/null; then
        kill "$MONITOR_PID" 2>/dev/null || true
        wait "$MONITOR_PID" 2>/dev/null || true
    fi
    if [[ -n "$DEVICE_LOCK" ]]; then
        "$SCRIPT_DIR/release-device.sh" "$DEVICE_LOCK"
    fi
    rm -f "$SERIAL_LOG"
}
trap cleanup EXIT

# ── 1. Claim device ─────────────────────────────────────────────────────
echo "── Claiming USB device ──"
eval "$("$SCRIPT_DIR/claim-device.sh")"
DEVICE_LOCK="$DEVICE_LOCK"  # exported by claim-device.sh
echo "Claimed $DEVICE_PORT (lock: $DEVICE_LOCK)"

# ── 2. Build firmware ───────────────────────────────────────────────────
if [[ "$SKIP_BUILD" == false ]]; then
    echo "── Building firmware ──"
    (cd "$REPO_ROOT" && cargo build)
else
    echo "── Skipping build (--skip-build) ──"
fi

# ── 3. Flash device ────────────────────────────────────────────────────
FIRMWARE="$REPO_ROOT/target/xtensa-esp32s2-espidf/debug/reconfigurable-device"
echo "── Flashing device on $DEVICE_PORT ──"
espflash flash --port "$DEVICE_PORT" "$FIRMWARE"

# ── 4. Read serial to discover Wi-Fi IP ──────────────────────────────
# Configure the serial port and start a background reader.
# espflash uses 115200 baud by default for the ESP32 monitor.
stty -F "$DEVICE_PORT" 115200 raw -echo 2>/dev/null || true
cat "$DEVICE_PORT" > "$SERIAL_LOG" 2>&1 &
MONITOR_PID=$!

echo "── Waiting for Wi-Fi IP (30 s timeout) ──"
DEADLINE=$((SECONDS + 30))
DEVICE_IP=""
while [[ $SECONDS -lt $DEADLINE ]]; do
    if IP_LINE=$(grep -m1 "Wi-Fi connected. IP:" "$SERIAL_LOG" 2>/dev/null); then
        DEVICE_IP=$(echo "$IP_LINE" | grep -oE '[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+')
        break
    fi
    sleep 1
done

if [[ -z "$DEVICE_IP" ]]; then
    echo "ERROR: device did not report an IP within 30 s" >&2
    echo "── Serial log tail ──" >&2
    tail -20 "$SERIAL_LOG" >&2
    exit 1
fi
echo "Device IP: $DEVICE_IP"

# Kill the serial reader — we only needed it for IP discovery
kill "$MONITOR_PID" 2>/dev/null || true
wait "$MONITOR_PID" 2>/dev/null || true
MONITOR_PID=""

# ── 5. Health check ─────────────────────────────────────────────────────
echo "── Health check: GET http://$DEVICE_IP/ (15 s timeout) ──"
DEADLINE=$((SECONDS + 15))
HEALTHY=false
while [[ $SECONDS -lt $DEADLINE ]]; do
    if curl -sf --max-time 5 "http://$DEVICE_IP/" > /dev/null 2>&1; then
        HEALTHY=true
        break
    fi
    sleep 1
done

if [[ "$HEALTHY" != true ]]; then
    echo "ERROR: HTTP health check failed within 15 s" >&2
    exit 1
fi
echo "Device is healthy."

# ── 6. Run pytest ────────────────────────────────────────────────────────
echo "── Running e2e tests ──"
export DEVICE_IP

# Load API_TOKEN from .env if present and not already set
if [[ -z "${API_TOKEN:-}" ]] && [[ -f "$REPO_ROOT/.env" ]]; then
    API_TOKEN=$(grep -E '^API_TOKEN=' "$REPO_ROOT/.env" | cut -d= -f2- | tr -d '"' || true)
    export API_TOKEN
fi

cd "$PROJECT_ROOT/tests/e2e"
uv sync --quiet
exec uv run pytest "${PYTEST_ARGS[@]+"${PYTEST_ARGS[@]}"}"
