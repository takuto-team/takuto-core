#!/bin/bash
# test-workflow.sh — Smoke test: auth, worktree, agent hello, cleanup.
# Runs as the maestro user inside the container (entrypoint handles root preamble).
set -euo pipefail

CONFIG_FILE="${MAESTRO_CONFIG:-/etc/maestro/config.toml}"
STEP=0
TOTAL=4
FAILED=0
TEST_BRANCH="test/maestro-smoketest"

# Read config values
base_branch=$(grep -E '^\s*base_branch\s*=' "$CONFIG_FILE" 2>/dev/null | sed 's/.*=\s*"\(.*\)"/\1/' | head -1 || true)
base_branch="${base_branch:-main}"
repo_path=$(grep -E '^\s*repo_path\s*=' "$CONFIG_FILE" 2>/dev/null | sed 's/.*=\s*"\(.*\)"/\1/' | head -1 || true)
repo_path="${repo_path:-/workspace}"
git_remote=$(grep -E '^\s*remote\s*=' "$CONFIG_FILE" 2>/dev/null | sed 's/.*=\s*"\(.*\)"/\1/' | head -1 || true)
git_remote="${git_remote:-origin}"
agent_provider=$(grep -E '^\s*provider\s*=' "$CONFIG_FILE" 2>/dev/null | sed 's/.*=\s*"\(.*\)"/\1/' | tr -d ' ' | head -1 || true)
agent_provider="${agent_provider:-claude}"

WORKTREE_PATH="$repo_path/worktrees/test-maestro-smoketest"

step_start() {
    STEP=$((STEP + 1))
    echo ""
    echo "--- Step $STEP/$TOTAL: $1 ---"
}

step_ok() {
    echo "[OK] Step $STEP passed: $1"
}

step_fail() {
    echo "[FAIL] Step $STEP failed: $1" >&2
    FAILED=1
}

# Always clean up on exit
cleanup() {
    echo ""
    echo "--- Cleanup ---"
    cd "$repo_path" 2>/dev/null || true
    if [ -d "$WORKTREE_PATH" ]; then
        echo "Removing worktree $WORKTREE_PATH..."
        git worktree remove --force "$WORKTREE_PATH" 2>/dev/null || rm -rf "$WORKTREE_PATH" 2>/dev/null || true
    fi
    if git rev-parse --verify "$TEST_BRANCH" >/dev/null 2>&1; then
        echo "Deleting branch $TEST_BRANCH..."
        git branch -D "$TEST_BRANCH" 2>/dev/null || true
    fi
    git worktree prune 2>/dev/null || true
    echo "Cleanup done."
    if [ "$FAILED" -eq 0 ]; then
        echo ""
        echo "=== All tests passed ==="
    else
        echo ""
        echo "=== Test failed ==="
        exit 1
    fi
}
trap cleanup EXIT

git config --global --add safe.directory "$repo_path" 2>/dev/null || true
gh auth setup-git 2>/dev/null || true

# Restore .claude.json from backup if missing (volume can lose it on unclean shutdown)
if [ ! -f "$HOME/.claude.json" ]; then
    backup=$(ls -t "$HOME/.claude/backups/.claude.json.backup."* 2>/dev/null | head -1)
    if [ -n "$backup" ]; then
        cp "$backup" "$HOME/.claude.json"
    fi
fi

echo "=== Maestro Smoke Test ==="
echo "Provider: $agent_provider | Base: $git_remote/$base_branch | Repo: $repo_path"

# Step 1: Preflight auth checks
step_start "Auth preflight"
auth_ok=true
if ! gh auth status >/dev/null 2>&1; then
    echo "  GitHub CLI: NOT authenticated" >&2
    auth_ok=false
else
    echo "  GitHub CLI: OK"
fi
if ! acli jira auth status >/dev/null 2>&1; then
    echo "  Atlassian CLI: NOT authenticated" >&2
    auth_ok=false
else
    echo "  Atlassian CLI: OK"
fi
if [ "$agent_provider" = "claude" ]; then
    if [ -n "${CLAUDE_CODE_OAUTH_TOKEN:-}" ]; then
        echo "  Claude Code: OK (CLAUDE_CODE_OAUTH_TOKEN set)"
    elif claude auth status >/dev/null 2>&1; then
        echo "  Claude Code: OK"
    else
        echo "  Claude Code: NOT authenticated" >&2
        auth_ok=false
    fi
elif [ "$agent_provider" = "cursor" ]; then
    if [ -n "${CURSOR_API_KEY:-}" ]; then
        echo "  Cursor Agent: OK (CURSOR_API_KEY set)"
    else
        echo "  Cursor Agent: cannot verify (no status command); will test in step 3"
    fi
fi
if [ "$auth_ok" = "false" ]; then
    step_fail "Auth preflight"
    exit 1
fi
step_ok "Auth preflight"

# Step 2: Create worktree
step_start "Create worktree"
cd "$repo_path"
echo "  Fetching $git_remote/$base_branch..."
git fetch "$git_remote" "$base_branch" --quiet
# Clean up any leftover from a previous failed run
if [ -d "$WORKTREE_PATH" ]; then
    echo "  Removing stale worktree..."
    git worktree remove --force "$WORKTREE_PATH" 2>/dev/null || rm -rf "$WORKTREE_PATH" 2>/dev/null || true
fi
if git rev-parse --verify "$TEST_BRANCH" >/dev/null 2>&1; then
    echo "  Removing stale branch..."
    git branch -D "$TEST_BRANCH" 2>/dev/null || true
fi
git worktree prune 2>/dev/null || true
mkdir -p "$(dirname "$WORKTREE_PATH")"
echo "  Creating worktree at $WORKTREE_PATH on branch $TEST_BRANCH..."
git worktree add -b "$TEST_BRANCH" "$WORKTREE_PATH" "$git_remote/$base_branch" --quiet
step_ok "Create worktree"

# Step 3: Run agent
step_start "Run agent ($agent_provider)"
if [ "$agent_provider" = "claude" ]; then
    model_flag=""
    model=$(grep -E '^\s*model\s*=' "$CONFIG_FILE" 2>/dev/null | sed 's/.*=\s*"\(.*\)"/\1/' | head -1 || true)
    if [ -n "$model" ]; then
        model_flag="--model $model"
    fi
    echo "  Running: claude --print -p 'Say hello' in worktree..."
    # shellcheck disable=SC2086
    if claude --dangerously-skip-permissions --print -p "Say hello" $model_flag -d "$WORKTREE_PATH" 2>&1; then
        step_ok "Run agent"
    else
        step_fail "Agent exited with error"
    fi
elif [ "$agent_provider" = "cursor" ]; then
    cursor_cli=$(grep -E '^\s*cursor_cli\s*=' "$CONFIG_FILE" 2>/dev/null | sed 's/.*=\s*"\(.*\)"/\1/' | head -1 || true)
    cursor_cli="${cursor_cli:-agent}"
    echo "  Running: $cursor_cli --print -p 'Say hello' in worktree..."
    if "$cursor_cli" --print -p "Say hello" -d "$WORKTREE_PATH" 2>&1; then
        step_ok "Run agent"
    else
        step_fail "Agent exited with error"
    fi
else
    step_fail "Unknown agent provider: $agent_provider"
fi

# Step 4: Cleanup (handled by trap)
step_start "Cleanup"
step_ok "Cleanup (runs via exit trap)"
