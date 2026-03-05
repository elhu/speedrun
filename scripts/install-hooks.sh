#!/bin/bash
#
# Install git hooks for pre-commit checks.
# Run this script after cloning: bash scripts/install-hooks.sh
# This script is idempotent and safe to run multiple times.
#

set -e

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# Handle both regular git repos and git worktrees
if [ -f "$REPO_ROOT/.git" ]; then
    # Git worktree - read the gitdir path from .git file
    GIT_DIR="$(cat "$REPO_ROOT/.git" | sed 's/^gitdir: //')"
    # For worktrees, hooks are in the common git directory
    COMMON_DIR="$(cat "$GIT_DIR/commondir")"
    HOOKS_DIR="$GIT_DIR/$COMMON_DIR/hooks"
elif [ -d "$REPO_ROOT/.git" ]; then
    # Regular git repo
    HOOKS_DIR="$REPO_ROOT/.git/hooks"
else
    echo "❌ Error: Not a git repository."
    exit 1
fi
PRE_COMMIT_HOOK="$HOOKS_DIR/pre-commit"
PRE_COMMIT_SCRIPT="$REPO_ROOT/scripts/pre-commit"

echo "🔧 Installing git hooks..."

# Ensure .git/hooks directory exists
if [ ! -d "$HOOKS_DIR" ]; then
    echo "❌ Error: .git/hooks directory not found. Are you in a git repository?"
    exit 1
fi

# Check if pre-commit hook already exists and points to our script
if [ -L "$PRE_COMMIT_HOOK" ]; then
    CURRENT_TARGET="$(readlink "$PRE_COMMIT_HOOK")"
    if [ "$CURRENT_TARGET" = "$PRE_COMMIT_SCRIPT" ]; then
        echo "✅ Pre-commit hook is already installed and up to date."
        exit 0
    else
        echo "⚠️  Pre-commit hook exists but points to different script: $CURRENT_TARGET"
        echo "   Removing existing hook and installing our hook..."
        rm "$PRE_COMMIT_HOOK"
    fi
elif [ -f "$PRE_COMMIT_HOOK" ]; then
    echo "⚠️  Pre-commit hook exists as a regular file. Backing up and replacing..."
    mv "$PRE_COMMIT_HOOK" "$PRE_COMMIT_HOOK.backup"
fi

# Create symlink to our pre-commit script
ln -s "$PRE_COMMIT_SCRIPT" "$PRE_COMMIT_HOOK"

echo "✅ Pre-commit hook installed successfully!"
echo "   Hook location: $PRE_COMMIT_HOOK"
echo "   Points to: $PRE_COMMIT_SCRIPT"
echo ""
echo "🎉 Git hooks are now active. All commits will run formatting, linting, and testing checks."