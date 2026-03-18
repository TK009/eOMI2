#!/usr/bin/env bash
# Set up the ESP toolchain and environment for building.
#
# Supports running from a git worktree: detects REPO_ROOT and symlinks .env
# from there when it is missing locally. Skips installation steps that are
# already satisfied so repeated calls are fast.
#
# Source this file (don't execute it) so that environment variables
# (export-esp.sh) propagate to the caller:
#   . ./scripts/setup-esp.sh

# Save caller's shell options and enforce strict mode for this script.
# Restored at the end so sourcing doesn't bleed into the caller.
_setup_esp_oldopts=$(set +o)
set -euo pipefail

. "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/_common.sh"

# ── 1. Install espup if missing ─────────────────────────────────────────
if ! command -v espup &>/dev/null; then
    echo "── Installing espup ──"
    cargo install espup --locked
else
    echo "── espup already installed ──"
fi

# ── 2. Install the ESP Rust toolchain if missing ────────────────────────
if [[ ! -f "$HOME/export-esp.sh" ]]; then
    echo "── Installing ESP toolchain ──"
    espup install
else
    echo "── ESP toolchain already installed ──"
fi

# ── 3. Source the ESP environment ────────────────────────────────────────
. "$HOME/export-esp.sh"

# ── 4. Set the rustup override for this project dir ─────────────────────
rustup override set esp

# ── 5. Symlink .env from REPO_ROOT or RIG_ROOT when running in a worktree ─
if [[ ! -e "$PROJECT_ROOT/.env" ]]; then
    if [[ "$PROJECT_ROOT" != "$REPO_ROOT" ]] && [[ -f "$REPO_ROOT/.env" ]]; then
        echo "── Symlinking .env from repo root ──"
        ln -s "$REPO_ROOT/.env" "$PROJECT_ROOT/.env"
    elif [[ -f "${RIG_ROOT:-}/.env" ]]; then
        echo "── Symlinking .env from rig root ──"
        ln -s "$RIG_ROOT/.env" "$PROJECT_ROOT/.env"
    fi
fi

# ── 6. Generate sdkconfig fragment with absolute partition table path ──
# ESP-IDF cmake resolves PARTITION_TABLE_CUSTOM_FILENAME relative to its
# own build output dir, not the project root. We generate a fragment with
# the absolute path and prepend it to ESP_IDF_SDKCONFIG_DEFAULTS so esp-idf-sys
# picks it up. "Last wins" in sdkconfig defaults, so our project sdkconfig.defaults
# (which is appended by embuild) will still override — but the partition path
# in sdkconfig.defaults uses a relative path that won't resolve. We put the
# absolute-path fragment AFTER sdkconfig.defaults by setting the env var.
_part_frag="$PROJECT_ROOT/target/sdkconfig.partitions.defaults"
mkdir -p "$PROJECT_ROOT/target"
cat > "$_part_frag" <<SDKEOF
CONFIG_PARTITION_TABLE_CUSTOM=y
CONFIG_PARTITION_TABLE_CUSTOM_FILENAME="$PROJECT_ROOT/partitions.csv"
CONFIG_PARTITION_TABLE_FILENAME="$PROJECT_ROOT/partitions.csv"
SDKEOF
export ESP_IDF_SDKCONFIG_DEFAULTS="$PROJECT_ROOT/sdkconfig.defaults;$_part_frag"
unset _part_frag

# Restore caller's shell options
eval "$_setup_esp_oldopts"
unset _setup_esp_oldopts
