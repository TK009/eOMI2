#!/usr/bin/env bash
# Build production firmware with maximum flash optimization.
#
# Compared to a regular release build, this:
#   - Uses the "production" Cargo profile (inherits release + strip)
#   - Disables std's backtrace feature (~130 KB savings: no gimli/addr2line/DWARF)
#   - Produces binary at target/xtensa-esp32s2-espidf/production/reconfigurable-device
#
# Usage:
#   ./scripts/build-production.sh              # standard production build
#   ./scripts/build-production.sh --features gpio  # with extra features

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Source ESP toolchain
. "$SCRIPT_DIR/setup-esp.sh"

cd "$PROJECT_ROOT"

# Workaround: esp-idf-sys's cmake resolves partitions.csv relative to its own
# output directory, not the project root.  Copy it so clean builds succeed.
# (This is a known issue with esp-idf-sys + custom partition tables.)
#
# On a fresh clone the esp-idf-sys output dirs don't exist yet.  In that case
# we run the build once (it will fail at the cmake stage), then copy the file
# into the newly-created dirs and retry.
_fix_partitions() {
    local found=false
    for dir in target/xtensa-esp32s2-espidf/*/build/esp-idf-sys-*/out/; do
        if [ -d "$dir" ]; then
            cp -n partitions.csv "$dir/partitions.csv" 2>/dev/null || true
            found=true
        fi
    done
    $found
}

CARGO_CMD=(cargo build --profile production --config 'unstable.build-std-features=[]' "$@")

# Try to fix partition paths before building
_fix_partitions 2>/dev/null || true

# Disable std's backtrace feature to exclude gimli/addr2line (~130 KB savings).
if "${CARGO_CMD[@]}" 2>&1; then
    exit 0
fi

# Build failed — if esp-idf-sys dirs now exist, copy partitions.csv and retry.
if _fix_partitions; then
    echo "── Retrying after copying partitions.csv to build dir ──"
    exec "${CARGO_CMD[@]}"
else
    echo "ERROR: build failed and no esp-idf-sys output dirs found" >&2
    exit 1
fi
