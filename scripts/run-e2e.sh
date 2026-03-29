#!/usr/bin/env bash
# Orchestrate an end-to-end test run against a real ESP32 device.
#
# Steps:
#   1. Ensure ESP toolchain is installed (setup-esp.sh)
#   2. Build firmware locally (unless --skip-build)
#   3. Build OTA firmware binaries A and B
#   4. Run memory budget check
#   5. Build WiFi bridge firmware
#   6. Run the pytest e2e suite (pytest handles device claiming, flashing,
#      IP discovery, and health checks via session-scoped fixtures)
#
# Device management is handled entirely by pytest fixtures (conftest.py +
# device_lock.py), which talk to the HTTP lock server.  This means only
# the devices actually needed by the selected tests are claimed — running
# a single-device subset (e.g. -k test_boot) won't lock the bridge.
#
# Usage:
#   ./scripts/run-e2e.sh                   # full run
#   ./scripts/run-e2e.sh --skip-build      # reuse existing firmware
#   ./scripts/run-e2e.sh -- -k test_boot   # pass extra args to pytest
#
# Environment:
#   CLAIM_DEVICES    — pin to specific device(s), e.g. "/dev/ttyUSB0"
#   DEVICE_LOCK_URL  — lock server URL (default: http://localhost:7357)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Source shared variables (PROJECT_ROOT, REPO_ROOT) and ESP environment.
# setup-esp.sh sources _common.sh and export-esp.sh, so their env vars
# propagate here.
. "$SCRIPT_DIR/setup-esp.sh"

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

# ── Source build-time env vars from .env ─────────────────────────────────
# EOMI_BOARD is needed at build time by build.rs to select board config.
for _envfile in "$PROJECT_ROOT/.env" "$REPO_ROOT/.env" "${RIG_ROOT:-}/.env"; do
    if [[ -f "$_envfile" ]] && [[ -z "${EOMI_BOARD:-}" ]]; then
        _val=$(grep -E '^EOMI_BOARD=' "$_envfile" | head -1 | cut -d= -f2- | tr -d "'\"") || true
        if [[ -n "$_val" ]]; then export EOMI_BOARD="$_val"; fi
    fi
done
unset _envfile _val

# ── 1. Build firmware ───────────────────────────────────────────────────
# Workaround: esp-idf-sys's cmake resolves partitions.csv relative to its own
# output directory.  Copy it so builds succeed after target/ changes.
for _dir in "$PROJECT_ROOT"/target/xtensa-esp32s2-espidf/*/build/esp-idf-sys-*/out/; do
    [[ -d "$_dir" ]] && cp -n "$PROJECT_ROOT/partitions.csv" "$_dir/partitions.csv" 2>/dev/null || true
done

BUILD_FEATURES="std,esp,gpio,lite-json,scripting,mem-stats"

if [[ "$SKIP_BUILD" == false ]]; then
    echo "── Building firmware ──"
    if ! (cd "$PROJECT_ROOT" && cargo build --no-default-features --features "$BUILD_FEATURES"); then
        echo "ERROR: firmware build failed" >&2
        exit 1
    fi
else
    echo "── Skipping build (--skip-build) ──"
fi

# ── 2. Build OTA binaries (firmware A and B) ─────────────────────────
# OTA binaries use release profile — debug builds (~2 MB) exceed the OTA
# partition size (0x1E0000 = 1920 KB).
FIRMWARE="$PROJECT_ROOT/target/xtensa-esp32s2-espidf/debug/reconfigurable-device"
OTA_FIRMWARE="$PROJECT_ROOT/target/xtensa-esp32s2-espidf/release/reconfigurable-device"
FIRMWARE_A_BIN="$PROJECT_ROOT/target/xtensa-esp32s2-espidf/release/firmware-a.bin"
FIRMWARE_B_BIN="$PROJECT_ROOT/target/xtensa-esp32s2-espidf/release/firmware-b.bin"

echo "── Building OTA binaries (release) ──"
# Build release version "A" for OTA
if ! (cd "$PROJECT_ROOT" && cargo build --release \
    --no-default-features --features "$BUILD_FEATURES"); then
    echo "ERROR: firmware A release build failed" >&2
    exit 1
fi
espflash save-image --chip esp32s2 --format esp-idf "$OTA_FIRMWARE" "$FIRMWARE_A_BIN"
gzip -c "$FIRMWARE_A_BIN" > "$FIRMWARE_A_BIN.gz"

# Build release version "B" with a different FIRMWARE_VERSION for OTA test
if ! (cd "$PROJECT_ROOT" && FIRMWARE_VERSION="e2e-ota-test" cargo build --release \
    --no-default-features --features "$BUILD_FEATURES"); then
    echo "ERROR: firmware B release build failed" >&2
    exit 1
fi
espflash save-image --chip esp32s2 --format esp-idf "$OTA_FIRMWARE" "$FIRMWARE_B_BIN"
gzip -c "$FIRMWARE_B_BIN" > "$FIRMWARE_B_BIN.gz"

export OTA_FIRMWARE_A_GZ="$FIRMWARE_A_BIN.gz"
export OTA_FIRMWARE_B_GZ="$FIRMWARE_B_BIN.gz"
# Raw (uncompressed) bin paths — preferred over gzip for ESP32-S2 OTA
# because the gzip decompressor needs ~43 KB heap that may not be available.
export OTA_FIRMWARE_A_BIN="$FIRMWARE_A_BIN"
export OTA_FIRMWARE_B_BIN="$FIRMWARE_B_BIN"
echo "OTA firmware A: $OTA_FIRMWARE_A_BIN"
echo "OTA firmware B: $OTA_FIRMWARE_B_BIN"

# ── 3. Memory budget check ──────────────────────────────────────────────
echo "── Memory budget check ──"
if ! (cd "$PROJECT_ROOT" && cargo test --target x86_64-unknown-linux-gnu --no-default-features --features std,lite-json,scripting --test memory_budget -- --nocapture); then
    echo "ERROR: memory budget exceeded — aborting before flash" >&2
    exit 1
fi

# ── 4. Build WiFi bridge firmware ──────────────────────────────────────
BRIDGE_FW_DIR="$PROJECT_ROOT/tests/e2e/bridge-fw"
BRIDGE_FIRMWARE="$BRIDGE_FW_DIR/target/xtensa-esp32s2-espidf/debug/wifi-bridge"
if [[ "$SKIP_BUILD" == false ]]; then
    echo "── Building bridge firmware ──"
    if (cd "$BRIDGE_FW_DIR" && cargo build); then
        export BRIDGE_FIRMWARE
        echo "Bridge firmware: $BRIDGE_FIRMWARE"
    else
        echo "WARNING: bridge build failed — provisioning tests will skip" >&2
    fi
else
    echo "── Skipping bridge build (--skip-build) ──"
    if [[ -f "$BRIDGE_FIRMWARE" ]]; then
        export BRIDGE_FIRMWARE
    fi
fi

# ── 5. Export paths and env vars for pytest ─────────────────────────────
# FIRMWARE_PATH tells conftest.py where the DUT firmware binary is.
# Respect existing FIRMWARE_PATH (e.g. to test with release firmware).
export FIRMWARE_PATH="${FIRMWARE_PATH:-$FIRMWARE}"

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

# ── 6. Run pytest ────────────────────────────────────────────────────────
echo "── Running e2e tests ──"
cd "$PROJECT_ROOT/tests/e2e"
uv sync --quiet
exec uv run pytest "${PYTEST_ARGS[@]+"${PYTEST_ARGS[@]}"}"
