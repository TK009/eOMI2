#!/usr/bin/env bash
# Orchestrate an end-to-end test run against a real ESP32 device.
#
# Steps:
#   0. Ensure ESP toolchain is installed (setup-esp.sh)
#   1. Claim a USB device from the pool (wait up to 240 s)
#   2. Build firmware locally (unless --skip-build)
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
#
# Environment:
#   CLAIM_DEVICES  — pin to specific device(s), e.g. "/dev/ttyUSB0"

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Source shared variables (PROJECT_ROOT, REPO_ROOT) and ESP environment.
# setup-esp.sh sources _common.sh and export-esp.sh, so their env vars
# propagate here.
. "$SCRIPT_DIR/setup-esp.sh"

SERIAL_LOG=$(mktemp /tmp/e2e-serial.XXXXXX)
MONITOR_PID=""
DEVICE_PORT=""
DEVICE_FD=""
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

# ── Helpers ──────────────────────────────────────────────────────────────
stop_monitor() {
    if [[ -n "$MONITOR_PID" ]] && kill -0 "$MONITOR_PID" 2>/dev/null; then
        kill "$MONITOR_PID" 2>/dev/null || true
        wait "$MONITOR_PID" 2>/dev/null || true
    fi
    MONITOR_PID=""
}

# ── Cleanup trap ─────────────────────────────────────────────────────────
cleanup() {
    echo "── Cleaning up ──"
    stop_monitor
    if [[ -n "${DEVICE_FD:-}" ]]; then
        . "$SCRIPT_DIR/release-device.sh"
    fi
    rm -f "$SERIAL_LOG"
}
trap cleanup EXIT

# ── 1. Claim device (wait up to 240 s) ──────────────────────────────────
echo "── Claiming USB device ──"
# shellcheck disable=SC2034  # consumed by sourced _claim-wait.sh
CLAIM_TIMEOUT=240
. "$SCRIPT_DIR/_claim-wait.sh"
echo "Claimed $DEVICE_PORT (fd: $DEVICE_FD)"

# ── 1b. Source build-time env vars from .env ──────────────────────────────
# EOMI_BOARD is needed at build time by build.rs to select board config.
for _envfile in "$PROJECT_ROOT/.env" "$REPO_ROOT/.env" "${RIG_ROOT:-}/.env"; do
    if [[ -f "$_envfile" ]] && [[ -z "${EOMI_BOARD:-}" ]]; then
        _val=$(grep -E '^EOMI_BOARD=' "$_envfile" | head -1 | cut -d= -f2- | tr -d "'\"") || true
        if [[ -n "$_val" ]]; then export EOMI_BOARD="$_val"; fi
    fi
done
unset _envfile _val

# ── 2. Build firmware ───────────────────────────────────────────────────
if [[ "$SKIP_BUILD" == false ]]; then
    echo "── Building firmware ──"
    if ! (cd "$PROJECT_ROOT" && cargo build --no-default-features --features std,esp,gpio,lite-json,scripting,mem-stats); then
        echo "ERROR: firmware build failed" >&2
        exit 1
    fi
else
    echo "── Skipping build (--skip-build) ──"
fi

# ── 2a. Build OTA binaries (firmware A and B) ─────────────────────────
FIRMWARE="$PROJECT_ROOT/target/xtensa-esp32s2-espidf/debug/reconfigurable-device"
FIRMWARE_A_BIN="$PROJECT_ROOT/target/xtensa-esp32s2-espidf/debug/firmware-a.bin"
FIRMWARE_B_BIN="$PROJECT_ROOT/target/xtensa-esp32s2-espidf/debug/firmware-b.bin"
BUILD_FEATURES="std,esp,gpio,lite-json,scripting,mem-stats"

echo "── Building OTA binaries ──"
# Save version "A" (current build) as OTA binary for restore step
espflash save-image --chip esp32s2 --format esp-idf "$FIRMWARE" "$FIRMWARE_A_BIN"
gzip -c "$FIRMWARE_A_BIN" > "$FIRMWARE_A_BIN.gz"

# Build version "B" with a different FIRMWARE_VERSION for OTA test
if ! (cd "$PROJECT_ROOT" && FIRMWARE_VERSION="e2e-ota-test" cargo build \
    --no-default-features --features "$BUILD_FEATURES"); then
    echo "ERROR: firmware B build failed" >&2
    exit 1
fi
espflash save-image --chip esp32s2 --format esp-idf "$FIRMWARE" "$FIRMWARE_B_BIN"
gzip -c "$FIRMWARE_B_BIN" > "$FIRMWARE_B_BIN.gz"

# Rebuild original version "A" to leave the build dir clean
if ! (cd "$PROJECT_ROOT" && unset FIRMWARE_VERSION && cargo build \
    --no-default-features --features "$BUILD_FEATURES"); then
    echo "ERROR: firmware A rebuild failed" >&2
    exit 1
fi

export OTA_FIRMWARE_A_GZ="$FIRMWARE_A_BIN.gz"
export OTA_FIRMWARE_B_GZ="$FIRMWARE_B_BIN.gz"
echo "OTA firmware A: $OTA_FIRMWARE_A_GZ"
echo "OTA firmware B: $OTA_FIRMWARE_B_GZ"

# ── 2b. Memory budget check ──────────────────────────────────────────────
echo "── Memory budget check ──"
if ! (cd "$PROJECT_ROOT" && cargo test --target x86_64-unknown-linux-gnu --no-default-features --features std,lite-json,scripting --test memory_budget -- --nocapture); then
    echo "ERROR: memory budget exceeded — aborting before flash" >&2
    exit 1
fi

# ── 3. Flash device and start serial capture ────────────────────────────
echo "── Flashing device on $DEVICE_PORT ──"
espflash flash --port "$DEVICE_PORT" "$FIRMWARE"

# Start serial capture immediately after flash returns.  The device is now
# rebooting; ESP32 boot (bootloader → Wi-Fi scan → DHCP) takes several
# seconds, so there is no realistic race with the IP log line.
if ! stty -F "$DEVICE_PORT" 115200 raw -echo 2>/dev/null; then
    echo "WARNING: stty failed — serial baud rate may be incorrect" >&2
fi
cat "$DEVICE_PORT" > "$SERIAL_LOG" 2>&1 &
MONITOR_PID=$!

# ── 4. Wait for Wi-Fi IP ────────────────────────────────────────────────
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
stop_monitor

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
export DEVICE_PORT

# Locate .env: project root > repo root > rig root (Gas Town worktrees).
ENV_FILE=""
if [[ -f "$PROJECT_ROOT/.env" ]]; then
    ENV_FILE="$PROJECT_ROOT/.env"
elif [[ -f "$REPO_ROOT/.env" ]]; then
    ENV_FILE="$REPO_ROOT/.env"
elif [[ -f "${RIG_ROOT:-}/.env" ]]; then
    ENV_FILE="$RIG_ROOT/.env"
fi

# Helper: read a KEY=VALUE from ENV_FILE, strip surrounding quotes.
_env_val() {
    local key="$1"
    if [[ -z "$ENV_FILE" ]]; then return 1; fi
    local raw
    raw=$(grep -E "^${key}=" "$ENV_FILE" | head -1) || return 1
    local val="${raw#*=}"
    case "$val" in
        \"*\") val="${val#\"}"; val="${val%\"}" ;;
        \'*\') val="${val#\'}"; val="${val%\'}" ;;
    esac
    printf '%s' "$val"
}

# Export WIFI_SSID, WIFI_PASS, API_TOKEN from .env if not already set.
for _key in WIFI_SSID WIFI_PASS API_TOKEN GPIO_OUT_PATH GPIO_IN_PATH ANALOG_IN_PATH PWM_PATH; do
    if [[ -z "${!_key:-}" ]]; then
        if _val=$(_env_val "$_key"); then
            export "$_key=$_val"
        fi
    fi
done
unset _key _val

cd "$PROJECT_ROOT/tests/e2e"
uv sync --quiet
exec uv run pytest "${PYTEST_ARGS[@]+"${PYTEST_ARGS[@]}"}"
