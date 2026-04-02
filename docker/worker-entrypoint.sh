#!/bin/bash
set -euo pipefail
if iptables -L -n >/dev/null 2>&1; then
    /usr/local/bin/egress-rules.sh
fi
[ -f /etc/maestro/env ] && set -a && . /etc/maestro/env && set +a
exec runuser -u maestro -- "$@"
