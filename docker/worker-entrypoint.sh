#!/bin/bash
set -euo pipefail
if iptables -L -n >/dev/null 2>&1; then
    /usr/local/bin/egress-rules.sh
fi
# Ensure shared volumes are writable by maestro (fresh volumes start root-owned)
for d in /home/maestro/.npm /home/maestro/.npm-global /home/maestro/.cache/mise /home/maestro/.local/share/mise; do
    [ -d "$d" ] && chown -R maestro:maestro "$d" 2>/dev/null || true
done
[ -f /etc/maestro/env ] && set -a && . /etc/maestro/env && set +a
# Read the centralized GitHub App token written by Maestro's background service.
# Falls through to any ambient GH_TOKEN already in the environment.
GH_APP_TOKEN_FILE="/home/maestro/.config/gh/gh-app-token"
[ -f "$GH_APP_TOKEN_FILE" ] && export GH_TOKEN="$(cat "$GH_APP_TOKEN_FILE")"
exec runuser -u maestro -- "$@"
