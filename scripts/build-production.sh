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
for dir in target/xtensa-esp32s2-espidf/*/build/esp-idf-sys-*/out/; do
    [ -d "$dir" ] && cp -n partitions.csv "$dir/partitions.csv" 2>/dev/null || true
done

# Disable std's backtrace feature to exclude gimli/addr2line (~130 KB savings).
# This overrides the default build-std-features which includes "backtrace".
exec cargo build --profile production \
    --config 'unstable.build-std-features=[]' \
    "$@"
