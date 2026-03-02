#!/usr/bin/env bash
# Set up the ESP toolchain and environment for building.
#
# Supports running from a git worktree: detects REPO_ROOT and copies .env
# from there when it is missing locally. Skips installation steps that are
# already satisfied so repeated calls are fast.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Resolve the main repo root (differs from PROJECT_ROOT inside a worktree).
REPO_ROOT="$(cd "$(git -C "$PROJECT_ROOT" rev-parse --git-common-dir)/.." && pwd)"

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

# ── 5. Copy .env from REPO_ROOT when running in a worktree ──────────────
if [[ "$PROJECT_ROOT" != "$REPO_ROOT" ]] && [[ -f "$REPO_ROOT/.env" ]] && [[ ! -f "$PROJECT_ROOT/.env" ]]; then
    echo "── Copying .env from repo root ──"
    cp "$REPO_ROOT/.env" "$PROJECT_ROOT/.env"
fi
