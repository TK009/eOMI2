#!/usr/bin/env bash
# Run cargo-fuzz targets against the OMI parser, O-DF path parser, and
# mJS script engine.
#
# Usage:
#   ./scripts/run-fuzz.sh                    # smoke-test all targets (10 s each)
#   ./scripts/run-fuzz.sh --target fuzz_omi_parse   # run one target only
#   ./scripts/run-fuzz.sh --duration 300     # 5-minute runs
#   ./scripts/run-fuzz.sh -- -max_len=8192   # pass extra libfuzzer flags
#
# Environment:
#   FUZZ_TOOLCHAIN — Rust toolchain to use (default: nightly)

set -euo pipefail

. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_common.sh"

FUZZ_DIR="$PROJECT_ROOT/fuzz"
TOOLCHAIN="${FUZZ_TOOLCHAIN:-nightly}"
DURATION=10
TARGET=""
EXTRA_ARGS=()

ALL_TARGETS=(fuzz_omi_parse fuzz_odf_path fuzz_script_exec)

# ── Parse arguments ──────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
    case "$1" in
        --target)   TARGET="$2"; shift 2 ;;
        --duration) DURATION="$2"; shift 2 ;;
        --)         shift; EXTRA_ARGS=("$@"); break ;;
        *)          EXTRA_ARGS+=("$1"); shift ;;
    esac
done

# ── Validate target name if given ────────────────────────────────────────
if [[ -n "$TARGET" ]]; then
    found=false
    for t in "${ALL_TARGETS[@]}"; do
        if [[ "$t" == "$TARGET" ]]; then found=true; break; fi
    done
    if [[ "$found" != true ]]; then
        echo "ERROR: unknown target '$TARGET'" >&2
        echo "Available targets: ${ALL_TARGETS[*]}" >&2
        exit 1
    fi
fi

# ── Ensure nightly toolchain is available ────────────────────────────────
if ! rustup run "$TOOLCHAIN" rustc --version &>/dev/null; then
    echo "── Installing $TOOLCHAIN toolchain ──"
    rustup install "$TOOLCHAIN"
fi

# ── Ensure rust-src is available (needed for -Zbuild-std / sanitizers) ──
if ! rustup component list --toolchain "$TOOLCHAIN" --installed 2>/dev/null | grep -q rust-src; then
    echo "── Adding rust-src component ──"
    rustup component add rust-src --toolchain "$TOOLCHAIN"
fi

# ── Ensure cargo-fuzz is installed ───────────────────────────────────────
if ! cargo +"$TOOLCHAIN" fuzz --version &>/dev/null 2>&1; then
    echo "── Installing cargo-fuzz ──"
    cargo +"$TOOLCHAIN" install cargo-fuzz --locked
fi

# ── Run targets ──────────────────────────────────────────────────────────
run_target() {
    local name="$1"
    echo "── Fuzzing $name (${DURATION}s) ──"
    (cd "$FUZZ_DIR" && cargo +"$TOOLCHAIN" fuzz run "$name" \
        -- -max_total_time="$DURATION" "${EXTRA_ARGS[@]+"${EXTRA_ARGS[@]}"}")
    echo "── $name finished ──"
}

if [[ -n "$TARGET" ]]; then
    run_target "$TARGET"
else
    for t in "${ALL_TARGETS[@]}"; do
        run_target "$t"
    done
fi

echo "── All done ──"
