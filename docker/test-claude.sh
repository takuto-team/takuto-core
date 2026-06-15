#!/bin/bash
# Test Claude Code headless — run as takuto user inside container
set -e
echo "=== Claude Code test ==="
echo "User: $(whoami)"
echo "HOME: $HOME"
echo ""

echo "--- Auth status ---"
claude auth status 2>&1 || true
echo ""

echo "--- API key source check ---"
echo "ANTHROPIC_API_KEY set: $(test -n "$ANTHROPIC_API_KEY" && echo yes || echo no)"
echo "Claude config dir: $(ls -la $HOME/.claude/ 2>/dev/null | head -5 || echo 'not found')"
echo ""

echo "--- Network connectivity test ---"
echo -n "api.anthropic.com: "
curl -s -o /dev/null -w "%{http_code}" --connect-timeout 5 https://api.anthropic.com 2>/dev/null || echo "BLOCKED"
echo ""
echo -n "api.claude.ai: "
curl -s -o /dev/null -w "%{http_code}" --connect-timeout 5 https://api.claude.ai 2>/dev/null || echo "BLOCKED"
echo ""
echo -n "claude.ai: "
curl -s -o /dev/null -w "%{http_code}" --connect-timeout 5 https://claude.ai 2>/dev/null || echo "BLOCKED"
echo ""

echo "--- Simple Claude test ---"
echo "Running: claude --print -p 'Say hello in one word' --output-format text"
timeout 60 claude --print -p "Say hello in one word" --output-format text 2>&1 || echo "FAILED with exit code: $?"
echo ""
echo "--- Headless test with --allow-dangerously-skip-permissions ---"
echo "Running: claude --allow-dangerously-skip-permissions --print -p 'Say hi' --output-format text"
timeout 60 claude --allow-dangerously-skip-permissions --print -p "Say hi" --output-format text 2>&1 || echo "FAILED with exit code: $?"
echo ""
echo "=== Claude test complete ==="
