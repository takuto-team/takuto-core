#!/bin/bash
# Test npm ci in the worktree — run as maestro user inside container
set -e
echo "=== npm ci test ==="
echo "Node: $(node --version)"
echo "npm: $(npm --version)"
echo "User: $(whoami)"
echo "HOME: $HOME"
echo "PWD: $(pwd)"

WORKTREE="/workspace/worktrees/fix-nero-176"
if [ ! -d "$WORKTREE" ]; then
    echo "ERROR: Worktree not found at $WORKTREE"
    echo "Available worktrees:"
    ls /workspace/worktrees/ 2>/dev/null || echo "  (none)"
    exit 1
fi

cd "$WORKTREE"
echo "package.json exists: $(test -f package.json && echo yes || echo no)"
echo "package-lock.json exists: $(test -f package-lock.json && echo yes || echo no)"
echo "node_modules exists: $(test -d node_modules && echo yes || echo no)"
echo "Disk space:"
df -h /workspace | tail -1
echo "Memory:"
free -h 2>/dev/null || echo "free not available"
echo ""
echo "Running npm ci..."
npm ci 2>&1
echo ""
echo "=== npm ci result: $? ==="
