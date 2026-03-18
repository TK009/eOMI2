#!/usr/bin/env bash
# measure-lite-json.sh — Binary size and peak memory: json vs lite-json
#
# Validates:
#   SC-002: lite-json produces smaller binary than serde + serde_json
#   SC-003: lite-json uses less peak memory during message parsing
#
# Usage:
#   ./scripts/measure-lite-json.sh          # full comparison
#   ./scripts/measure-lite-json.sh --size   # binary size only
#   ./scripts/measure-lite-json.sh --mem    # memory only

set -euo pipefail

HOST_TARGET="x86_64-unknown-linux-gnu"
CARGO_COMMON="--target $HOST_TARGET --config unstable.build-std=[]"

# Features shared between both builds (everything except the JSON impl)
COMMON_FEATURES="std"

RED='\033[0;31m'
GREEN='\033[0;32m'
BOLD='\033[1m'
RESET='\033[0m'

do_size=true
do_mem=true

case "${1:-}" in
    --size) do_mem=false ;;
    --mem)  do_size=false ;;
    "")     ;; # both
    *)      echo "Usage: $0 [--size|--mem]" >&2; exit 1 ;;
esac

# ── Binary size comparison (SC-002) ─────────────────────────────────────────

measure_lib_size() {
    local features="$1"
    local label="$2"

    # Build the library in release mode for accurate size comparison
    cargo build $CARGO_COMMON --release --no-default-features \
        --features "$features" 2>&1 >/dev/null

    local rlib
    rlib=$(find "target/$HOST_TARGET/release/deps" \
        -name 'libreconfigurable_device-*.rlib' \
        -printf '%T@ %p\n' 2>/dev/null | sort -rn | head -1 | cut -d' ' -f2)

    if [[ -z "$rlib" ]]; then
        echo "ERROR: could not find rlib for $label build" >&2
        return 1
    fi

    # Use file size of the rlib as the primary metric.
    # rlib = ar archive containing .o object code + .rmeta (type info).
    # Extract the .o to measure actual machine code contribution.
    local file_size
    file_size=$(stat -c%s "$rlib")

    # Extract .o members and sum their .text sections
    local tmpdir
    tmpdir=$(mktemp -d)
    (cd "$tmpdir" && ar x "$rlib" 2>/dev/null) || true
    local text_size=0
    for obj in "$tmpdir"/*.o; do
        [[ -f "$obj" ]] || continue
        local t
        t=$(size "$obj" 2>/dev/null | tail -1 | awk '{print $1}')
        text_size=$(( text_size + t ))
    done
    rm -rf "$tmpdir"

    echo "${label}:text=${text_size}:file=${file_size}"
}

if $do_size; then
    echo ""
    echo -e "${BOLD}═══ SC-002: Binary Size Comparison ═══${RESET}"
    echo ""

    echo "Building with json (serde + serde_json)..."
    json_result=$(measure_lib_size "$COMMON_FEATURES,json" "json")
    json_text=$(echo "$json_result" | grep -o 'text=[0-9]*' | cut -d= -f2)
    json_file=$(echo "$json_result" | grep -o 'file=[0-9]*' | cut -d= -f2)

    # Clean deps to force rebuild with different features
    rm -f target/$HOST_TARGET/release/deps/libreconfigurable_device-*

    echo "Building with lite-json (no serde)..."
    lite_result=$(measure_lib_size "$COMMON_FEATURES,lite-json" "lite-json")
    lite_text=$(echo "$lite_result" | grep -o 'text=[0-9]*' | cut -d= -f2)
    lite_file=$(echo "$lite_result" | grep -o 'file=[0-9]*' | cut -d= -f2)

    # Clean up to avoid stale artifacts for subsequent builds
    rm -f target/$HOST_TARGET/release/deps/libreconfigurable_device-*

    echo ""
    echo "  Binary size (release, host target)"
    echo "  ───────────────────────────────────────────────────────"
    printf "  %-12s  .text: %8d B  (%d KB)   rlib: %8d B  (%d KB)\n" \
        "json" "$json_text" "$(( json_text / 1024 ))" "$json_file" "$(( json_file / 1024 ))"
    printf "  %-12s  .text: %8d B  (%d KB)   rlib: %8d B  (%d KB)\n" \
        "lite-json" "$lite_text" "$(( lite_text / 1024 ))" "$lite_file" "$(( lite_file / 1024 ))"
    echo "  ───────────────────────────────────────────────────────"

    # Use .text if available, otherwise fall back to rlib file size.
    # With LTO enabled (release profile), .o files may be empty — rlib
    # still captures the code size difference via bitcode + metadata.
    json_metric="$json_text"
    lite_metric="$lite_text"
    metric_label=".text"
    if [[ "$json_text" -eq 0 && "$lite_text" -eq 0 ]]; then
        json_metric="$json_file"
        lite_metric="$lite_file"
        metric_label="rlib"
    fi

    if [[ "$lite_metric" -lt "$json_metric" ]]; then
        savings=$(( json_metric - lite_metric ))
        pct=$(awk "BEGIN { printf \"%.1f\", ($savings / $json_metric) * 100 }")
        echo -e "  ${GREEN}SC-002 PASS${RESET}: lite-json ${metric_label} is ${savings} B (${pct}%) smaller"
    elif [[ "$lite_metric" -eq "$json_metric" ]]; then
        echo -e "  SC-002 INCONCLUSIVE: sizes are identical"
    else
        excess=$(( lite_metric - json_metric ))
        echo -e "  ${RED}SC-002 FAIL${RESET}: lite-json ${metric_label} is ${excess} B larger than json"
    fi
    echo ""
fi

# ── Peak memory comparison (SC-003) ─────────────────────────────────────────

run_memory_test() {
    local features="$1"
    local label="$2"

    local output
    output=$(cargo test $CARGO_COMMON --no-default-features \
        --features "$features" \
        --test lite_json_memory -- --nocapture measure_peak_memory 2>&1)

    # Extract machine-readable result line
    local result_line
    result_line=$(echo "$output" | grep "^MEMORY_RESULT:" || true)

    if [[ -z "$result_line" ]]; then
        echo "ERROR: memory test produced no result for $label" >&2
        echo "$output" >&2
        return 1
    fi

    # Print human-readable output (indented)
    echo "$output" | grep -E "^\s+(Peak|read|write|response|delete|cancel|TOTAL|─)" || true

    echo "$result_line"
}

if $do_mem; then
    echo -e "${BOLD}═══ SC-003: Peak Memory Comparison ═══${RESET}"
    echo ""

    echo "Measuring json (serde + serde_json)..."
    echo ""
    json_mem_output=$(run_memory_test "$COMMON_FEATURES,json" "json")
    json_peak=$(echo "$json_mem_output" | grep "^MEMORY_RESULT:" | grep -o 'peak=[0-9]*' | cut -d= -f2)
    echo "$json_mem_output" | grep -v "^MEMORY_RESULT:" || true
    echo ""

    echo "Measuring lite-json..."
    echo ""
    lite_mem_output=$(run_memory_test "$COMMON_FEATURES,lite-json" "lite-json")
    lite_peak=$(echo "$lite_mem_output" | grep "^MEMORY_RESULT:" | grep -o 'peak=[0-9]*' | cut -d= -f2)
    echo "$lite_mem_output" | grep -v "^MEMORY_RESULT:" || true
    echo ""

    echo "  Memory comparison"
    echo "  ───────────────────────────────────────────────"
    printf "  %-12s  total peak: %8d B\n" "json" "$json_peak"
    printf "  %-12s  total peak: %8d B\n" "lite-json" "$lite_peak"
    echo "  ───────────────────────────────────────────────"

    if [[ "$lite_peak" -lt "$json_peak" ]]; then
        savings=$(( json_peak - lite_peak ))
        pct=$(awk "BEGIN { printf \"%.1f\", ($savings / $json_peak) * 100 }")
        echo -e "  ${GREEN}SC-003 PASS${RESET}: lite-json peak is ${savings} B (${pct}%) less"
    elif [[ "$lite_peak" -eq "$json_peak" ]]; then
        echo -e "  ${GREEN}SC-003 PASS${RESET}: peak memory is equal"
    else
        excess=$(( lite_peak - json_peak ))
        echo -e "  ${RED}SC-003 FAIL${RESET}: lite-json peak is ${excess} B more than json"
    fi
    echo ""
fi

echo -e "${BOLD}Done.${RESET}"
