#!/usr/bin/env bash
# Generate code coverage reports for host tests using cargo-tarpaulin.
#
# Usage:
#   ./scripts/coverage.sh              # Print summary to stdout
#   ./scripts/coverage.sh --html       # Also generate HTML report in coverage/
#   ./scripts/coverage.sh --json       # Also generate JSON report in coverage/
#
# Requires: cargo-tarpaulin (install with: cargo install cargo-tarpaulin)

set -euo pipefail
. "$(dirname "$0")/_common.sh"
cd "$REPO_ROOT"

# Check that cargo-tarpaulin is installed
if ! cargo tarpaulin --version >/dev/null 2>&1; then
    echo "ERROR: cargo-tarpaulin is not installed." >&2
    echo "Install it with: cargo install cargo-tarpaulin" >&2
    exit 1
fi

# Parse arguments
OUTPUT_FLAGS=()
for arg in "$@"; do
    case "$arg" in
        --html)
            mkdir -p coverage
            OUTPUT_FLAGS+=(--out html --output-dir coverage)
            ;;
        --json)
            mkdir -p coverage
            OUTPUT_FLAGS+=(--out json --output-dir coverage)
            ;;
        --help|-h)
            echo "Usage: $0 [--html] [--json]"
            echo ""
            echo "  --html   Generate HTML report in coverage/"
            echo "  --json   Generate JSON report in coverage/"
            echo ""
            echo "Coverage summary is always printed to stdout."
            exit 0
            ;;
        *)
            echo "Unknown argument: $arg" >&2
            exit 1
            ;;
    esac
done

# The .cargo/config.toml sets build-std and defaults to the esp toolchain.
# Use the stable toolchain and clear build-std via cargo config override.
export RUSTUP_TOOLCHAIN=stable
exec cargo tarpaulin \
    --target x86_64-unknown-linux-gnu \
    --no-default-features \
    --features std,json,scripting \
    --skip-clean \
    --timeout 120 \
    --engine llvm \
    "${OUTPUT_FLAGS[@]}"
