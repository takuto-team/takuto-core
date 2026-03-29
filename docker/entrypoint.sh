#!/bin/bash
# entrypoint.sh — Container entrypoint for Maestro
#
# Applies egress rules if NET_ADMIN capability is available,
# then starts the Maestro binary.

set -euo pipefail

# Try to apply egress rules (requires NET_ADMIN capability)
if iptables -L -n >/dev/null 2>&1; then
    echo "NET_ADMIN capability detected, applying egress rules..."
    /usr/local/bin/egress-rules.sh
else
    echo "WARNING: NET_ADMIN capability not available. Egress rules NOT applied."
    echo "         Run container with --cap-add=NET_ADMIN to enable network restrictions."
fi

# Configure git safe directory for the workspace
git config --global --add safe.directory /workspace

# Start Maestro
exec /usr/local/bin/maestro "$@"
