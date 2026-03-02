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

# ── 5. Symlink .env from REPO_ROOT when running in a worktree ───────────
if [[ "$PROJECT_ROOT" != "$REPO_ROOT" ]] && [[ -f "$REPO_ROOT/.env" ]] && [[ ! -e "$PROJECT_ROOT/.env" ]]; then
    echo "── Symlinking .env from repo root ──"
    ln -s "$REPO_ROOT/.env" "$PROJECT_ROOT/.env"
fi
