#!/usr/bin/env bash
#
# Install local git hooks that mirror CI's gates. After running this,
# `git push` will refuse to push when the workspace has fmt/clippy/test
# drift or any other gate failure.
#
# The hook calls `scripts/preflight.sh` in fast mode (no docker, no
# network). Pass --full to your push to run everything CI runs:
#
#   PREFLIGHT_FULL=1 git push
#
# To disable the hook for one push:
#
#   git push --no-verify
#
# To uninstall: delete `.git/hooks/pre-push`.

set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

mkdir -p .git/hooks

cat > .git/hooks/pre-push <<'HOOK'
#!/usr/bin/env bash
# Auto-installed by scripts/install-git-hooks.sh.
# Runs the same gates CI runs on every PR. To bypass once,
# `git push --no-verify`.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
ARGS=()
if [[ "${PREFLIGHT_FULL:-0}" == "1" ]]; then
  ARGS+=(--full)
fi
exec ./scripts/preflight.sh "${ARGS[@]}"
HOOK

chmod +x .git/hooks/pre-push

echo "Installed pre-push hook → .git/hooks/pre-push"
echo
echo "Bypass once:           git push --no-verify"
echo "Run full set on push:  PREFLIGHT_FULL=1 git push"
