#!/bin/bash
# Test network egress from container
echo "=== Egress test ==="
echo "User: $(whoami)"
echo ""

hosts=(
    "api.anthropic.com"
    "api.claude.ai"
    "claude.ai"
    "console.anthropic.com"
    "cdn.anthropic.com"
    "statsig.anthropic.com"
    "statsig.claude.ai"
    "sentry.io"
    "github.com"
    "api.github.com"
    "registry.npmjs.org"
    "api.atlassian.com"
)

for host in "${hosts[@]}"; do
    printf "%-30s " "$host:"
    code=$(curl -s -o /dev/null -w "%{http_code}" --connect-timeout 5 "https://$host" 2>/dev/null)
    if [ -n "$code" ] && [ "$code" != "000" ]; then
        echo "OK ($code)"
    else
        echo "BLOCKED"
    fi
done

echo ""
echo "=== Egress test complete ==="
