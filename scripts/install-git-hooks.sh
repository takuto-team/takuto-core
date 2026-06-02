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
repo_root="$(pwd)"

# If `core.hooksPath` is set globally to a path that ISN'T this
# repo's own `.git/hooks/`, git ignores `<repo>/.git/hooks/pre-push`
# entirely. That breaks the install silently. Override locally so
# this repo's hooks are read from this repo's hook directory — a
# global setting pointing at someone else's repo is almost always
# unintentional in this context.
configured_hooks_path="$(git config --get core.hooksPath || true)"
expected_hooks_path="$repo_root/.git/hooks"
if [[ -n "$configured_hooks_path" && "$configured_hooks_path" != "$expected_hooks_path" ]]; then
  echo "core.hooksPath is set globally to: $configured_hooks_path"
  echo "Overriding locally for this repo so the hook lands where git reads it."
  git config --local --unset-all core.hooksPath || true
fi

hooks_dir="$expected_hooks_path"
mkdir -p "$hooks_dir"

cat > "$hooks_dir/pre-push" <<'HOOK'
#!/usr/bin/env bash
# Auto-installed by scripts/install-git-hooks.sh.
# Runs the same gates CI runs on every PR. To bypass once,
# `git push --no-verify`.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
if [[ "${PREFLIGHT_FULL:-0}" == "1" ]]; then
  exec ./scripts/preflight.sh --full
else
  exec ./scripts/preflight.sh
fi
HOOK

chmod +x "$hooks_dir/pre-push"

echo "Installed pre-push hook → $hooks_dir/pre-push"
echo
echo "Bypass once:           git push --no-verify"
echo "Run full set on push:  PREFLIGHT_FULL=1 git push"
