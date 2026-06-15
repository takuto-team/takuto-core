#!/usr/bin/env bash
#
# Checked push: run the full local gate (scripts/preflight.sh) and push only if
# every gate passes.
#
# Why a wrapper instead of a git pre-push hook: git opens the connection to the
# remote BEFORE running a pre-push hook, so a multi-minute hook leaves that
# connection idle until the server drops it — the subsequent object transfer
# then dies with SIGPIPE (exit 141) even though the checks passed. Running the
# gate here first and pushing afterward on a fresh connection avoids that. The
# push uses --no-verify because the gate already ran in this script (there is
# intentionally no pre-push hook installed).
#
# Usage:
#   ./scripts/checked-push.sh                       # push current branch to its upstream
#   ./scripts/checked-push.sh origin main v0.6.0    # explicit refs / tags
#   make checked-push                               # same, no args
#   make checked-push ARGS="origin main v0.6.0"     # with args
#
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

./scripts/preflight.sh

echo
echo "✓ Preflight passed — pushing (git push --no-verify $*)"
git push --no-verify "$@"
