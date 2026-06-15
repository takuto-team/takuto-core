#!/usr/bin/env bash
#
# Point git at the repo's tracked hooks directory (`.githooks/`) so the
# pre-push gate runs before every push. Idempotent — safe to re-run.
#
#   ./scripts/install-git-hooks.sh
#
# The pre-push hook runs ./scripts/preflight.sh (cargo fmt/clippy/test/doc,
# UI lint/test/build, license + config-doc). Bypass once with:
#   git push --no-verify
#
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

git config core.hooksPath .githooks
chmod +x .githooks/* 2>/dev/null || true

echo "✓ git hooks installed: core.hooksPath -> .githooks"
echo "  pre-push will run ./scripts/preflight.sh"
echo "  bypass once with: git push --no-verify"
