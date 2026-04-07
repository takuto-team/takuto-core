#!/bin/bash
# test-workflow.sh — Smoke test: auth, worktree, skill-driven workflow, cleanup.
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
    # Remove test skill
    rm -rf "$HOME/.claude/skills/say-hello" "$HOME/.cursor/skills/say-hello" 2>/dev/null || true
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

# Step 3: Run test workflow with skill interpolation
step_start "Run workflow ($agent_provider)"

# Create say-hello skill for both providers
SKILL_BODY='---
name: say-hello
description: Greet someone and mention the weather
---

Greet the person named $1 warmly. Mention that the weather is $2 today.
Keep the response to a single short sentence.
All arguments: $ARGUMENTS'

mkdir -p "$HOME/.claude/skills/say-hello" "$HOME/.cursor/skills/say-hello"
echo "$SKILL_BODY" > "$HOME/.claude/skills/say-hello/SKILL.md"
echo "$SKILL_BODY" > "$HOME/.cursor/skills/say-hello/SKILL.md"
echo "  Created say-hello skill for claude and cursor"

# Read model from config
model_flag=""
model=$(grep -E '^\s*model\s*=' "$CONFIG_FILE" 2>/dev/null | sed 's/.*=\s*"\(.*\)"/\1/' | head -1 || true)
if [ -n "$model" ]; then
    model_flag="--model $model"
fi

# --- Workflow step 1: /say-hello "John Doe" "Cold" ---
echo "  Workflow step 1/2: say-hello skill with args"

# Read SKILL.md, strip frontmatter, substitute args (replicates Maestro skill_resolve logic)
SKILL_CONTENT=$(sed '1{/^---$/d}' "$HOME/.claude/skills/say-hello/SKILL.md" | sed '1,/^---$/d')
SKILL_CONTENT=$(echo "$SKILL_CONTENT" | sed 's/\$ARGUMENTS/John Doe Cold/g; s/\$1/John Doe/g; s/\$2/Cold/g')

if [ "$agent_provider" = "claude" ]; then
    echo "  Running: claude --system-prompt <skill> -p 'Follow the instructions in the system prompt.'"
    # shellcheck disable=SC2086
    STEP1_OUTPUT=$(claude --dangerously-skip-permissions --print --verbose \
        -p "Follow the instructions in the system prompt." \
        --system-prompt "$SKILL_CONTENT" \
        --output-format stream-json \
        $model_flag \
        -d "$WORKTREE_PATH" 2>&1) || true

    # Extract session ID from init event
    SESSION_ID=$(echo "$STEP1_OUTPUT" | grep '"subtype":"init"' | head -1 | sed 's/.*"session_id":"\([^"]*\)".*/\1/' || true)
    # Extract result text
    STEP1_RESULT=$(echo "$STEP1_OUTPUT" | grep '"type":"result"' | head -1 | sed 's/.*"result":"\([^"]*\)".*/\1/' || true)
    echo "  Step 1 result: $STEP1_RESULT"

    if [ -z "$SESSION_ID" ]; then
        step_fail "Could not extract session ID from step 1"
    else
        echo "  Session ID: $SESSION_ID"

        # --- Workflow step 2: prompt "say goodbye" ---
        echo "  Workflow step 2/2: say goodbye (prompt only, resume session)"
        # shellcheck disable=SC2086
        STEP2_OUTPUT=$(claude --dangerously-skip-permissions --print --verbose \
            -p "Say goodbye." \
            --output-format stream-json \
            --resume "$SESSION_ID" \
            $model_flag \
            -d "$WORKTREE_PATH" 2>&1) || true

        STEP2_RESULT=$(echo "$STEP2_OUTPUT" | grep '"type":"result"' | head -1 | sed 's/.*"result":"\([^"]*\)".*/\1/' || true)
        echo "  Step 2 result: $STEP2_RESULT"

        # Verify both steps produced output
        if [ -n "$STEP1_RESULT" ] && [ -n "$STEP2_RESULT" ]; then
            step_ok "Workflow completed (2 steps, skill + prompt)"
        else
            step_fail "One or both workflow steps produced no output"
        fi
    fi

elif [ "$agent_provider" = "cursor" ]; then
    cursor_cli=$(grep -E '^\s*cursor_cli\s*=' "$CONFIG_FILE" 2>/dev/null | sed 's/.*=\s*"\(.*\)"/\1/' | head -1 || true)
    cursor_cli="${cursor_cli:-agent}"

    # Cursor: skills are invoked natively via /skill-name args
    echo "  Running: $cursor_cli --print -p '/say-hello John Doe Cold'"
    if "$cursor_cli" --print -p '/say-hello "John Doe" "Cold"' -d "$WORKTREE_PATH" 2>&1; then
        echo "  Workflow step 2/2: say goodbye"
        if "$cursor_cli" --print -p "Say goodbye." -d "$WORKTREE_PATH" 2>&1; then
            step_ok "Workflow completed (2 steps, skill + prompt)"
        else
            step_fail "Step 2 (say goodbye) failed"
        fi
    else
        step_fail "Step 1 (say-hello skill) failed"
    fi
else
    step_fail "Unknown agent provider: $agent_provider"
fi

# Step 4: Cleanup (handled by trap)
step_start "Cleanup"
step_ok "Cleanup (runs via exit trap)"
