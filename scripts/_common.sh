#!/usr/bin/env bash
# Shared variables for e2e scripts.
#
# Source this file; do not execute it.
#   . "$(dirname "$0")/_common.sh"

_COMMON_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$_COMMON_DIR/.." && pwd)"

# Resolve the main repo root (differs from PROJECT_ROOT inside a worktree).
# git-common-dir points to the shared .git directory; its parent is the root.
# In Gas Town bare-repo worktrees, git-common-dir points to the bare .repo.git,
# so its parent is the rig container, not the project root. Fall back to
# PROJECT_ROOT in that case since every worktree has a full working tree.
_git_common="$(git -C "$PROJECT_ROOT" rev-parse --git-common-dir)"
_rig_root="$(cd "$_git_common/.." && pwd)"
REPO_ROOT="$_rig_root"
if [[ ! -f "$REPO_ROOT/Cargo.toml" ]]; then
    REPO_ROOT="$PROJECT_ROOT"
fi
# RIG_ROOT: the rig container directory (parent of .repo.git).
# In a bare-repo worktree this differs from both PROJECT_ROOT and REPO_ROOT.
# Useful for rig-level config like .env that lives outside the project tree.
RIG_ROOT="$_rig_root"
unset _git_common _rig_root _COMMON_DIR

# Sanity check: REPO_ROOT must look like the project root.
if [[ ! -f "$REPO_ROOT/Cargo.toml" ]]; then
    echo "ERROR: REPO_ROOT ($REPO_ROOT) does not contain Cargo.toml" >&2
    exit 1
fi
