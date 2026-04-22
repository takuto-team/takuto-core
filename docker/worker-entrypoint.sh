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
exec runuser -u maestro -- "$@"
