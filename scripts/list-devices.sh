#!/usr/bin/env bash
# Show USB serial devices and their lock status.
#
# Usage:
#   ./scripts/list-devices.sh
#
# For each device, probes the actual flock state (not just lock file
# existence) so the output is accurate even when a holder crashed without
# cleaning up.

set -euo pipefail

# Lock directory: honour DEVICE_LOCK_DIR, then try git-common-dir, then cwd.
if [[ -n "${DEVICE_LOCK_DIR:-}" ]]; then
    lock_dir="$DEVICE_LOCK_DIR"
elif _gc="$(git rev-parse --git-common-dir 2>/dev/null)"; then
    lock_dir="$(cd "$_gc/.." && pwd)/.device-locks"
    unset _gc
else
    lock_dir="$(pwd)/.device-locks"
fi
mkdir -p "$lock_dir"

# ── Collect devices ──────────────────────────────────────────────────────
devices=()
for g in /dev/ttyUSB* /dev/ttyACM*; do
    [[ -e "$g" ]] && devices+=("$g")
done

if [[ ${#devices[@]} -eq 0 ]]; then
    echo "No USB serial devices found."
    exit 0
fi

# ── Check each device ───────────────────────────────────────────────────
locked=0
free=0

for dev in "${devices[@]}"; do
    base="${dev##*/}"
    lockfile="$lock_dir/${base}.lock"

    if [[ ! -f "$lockfile" ]]; then
        printf "  %-20s  FREE\n" "$dev"
        : $(( free += 1 ))
        continue
    fi

    # Probe the real flock state: acquire in a subshell (released on exit).
    if flock -n "$lockfile" true 2>/dev/null; then
        printf "  %-20s  FREE\n" "$dev"
        : $(( free += 1 ))
    else
        info=$(tr '\n' '  ' < "$lockfile" 2>/dev/null || echo "?")
        printf "  %-20s  LOCKED  %s\n" "$dev" "$info"
        : $(( locked += 1 ))
    fi
done

# ── Stale lock files (device removed) ───────────────────────────────────
stale=()
for lf in "$lock_dir"/*.lock; do
    [[ -f "$lf" ]] || continue
    base="$(basename "$lf" .lock)"
    [[ ! -e "/dev/$base" ]] && stale+=("$lf")
done

if [[ ${#stale[@]} -gt 0 ]]; then
    echo ""
    echo "Stale lock files (device removed):"
    for lf in "${stale[@]}"; do
        printf "  %s\n" "$(basename "$lf")"
    done
fi

echo ""
echo "Total: ${#devices[@]}  Locked: $locked  Free: $free"
