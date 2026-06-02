#!/usr/bin/env bash
#
# Local pre-push check. Runs the same gates CI runs on every PR, in
# the same order. Failures here mean the corresponding CI job will be
# red — fix them locally and re-run.
#
# Heavy/external gates (gitleaks scan, cargo-deny, cargo-audit, npm
# audit, container build) are skipped by default to keep the script
# fast and offline. Pass `--full` to run them too.
#
# Usage:
#   ./scripts/preflight.sh           # fast subset (no network / docker)
#   ./scripts/preflight.sh --full    # everything CI runs
#
# Install as a git pre-push hook: see scripts/install-git-hooks.sh

set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

FULL=0
for arg in "$@"; do
  case "$arg" in
    --full) FULL=1 ;;
    -h|--help)
      grep '^#' "$0" | sed 's/^# \?//'
      exit 0
      ;;
    *)
      echo "preflight: unknown argument '$arg'" >&2
      exit 2
      ;;
  esac
done

GREEN=$'\033[0;32m'
RED=$'\033[0;31m'
DIM=$'\033[0;2m'
RESET=$'\033[0m'

run() {
  local name="$1"
  shift
  printf '%b▶ %s%b\n' "$DIM" "$name" "$RESET"
  if "$@"; then
    printf '%b✓ %s%b\n\n' "$GREEN" "$name" "$RESET"
  else
    printf '%b✗ %s%b\n' "$RED" "$name" "$RESET"
    exit 1
  fi
}

# Some Rust crates embed `ui/dist/` via rust-embed. Build artifacts are
# gitignored; create an empty placeholder so cargo can compile cleanly
# even if `npm run build` hasn't run yet.
mkdir -p ui/dist
touch ui/dist/.gitkeep

# ── Fast Rust gates ───────────────────────────────────────────────
run "cargo fmt --all -- --check" \
  cargo fmt --all -- --check

run "cargo clippy --workspace --all-targets -- -D warnings" \
  cargo clippy --workspace --all-targets --locked -- -D warnings

run "cargo test --workspace --locked" \
  cargo test --workspace --locked

run "cargo doc --workspace --no-deps --locked" \
  cargo doc --workspace --no-deps --locked

# ── License headers ───────────────────────────────────────────────
run "license headers (FSL)" \
  ./scripts/check-license-headers.sh

# ── Config doc ────────────────────────────────────────────────────
run "config documentation" \
  ./scripts/check-config-doc.sh

# ── UI gates ──────────────────────────────────────────────────────
if [[ -f ui/package.json ]]; then
  (
    cd ui

    # `npm ci` rebuilds node_modules from lockfile. Slow on cold cache;
    # only run when the lockfile or package.json is newer than node_modules.
    if [[ ! -d node_modules ]] || \
       [[ package-lock.json -nt node_modules ]] || \
       [[ package.json -nt node_modules ]]; then
      run "npm ci" npm ci
    fi

    run "ui: npm run lint" npm run lint
    run "ui: npm test"     npm test
    run "ui: tsc --noEmit" npx tsc --noEmit
    run "ui: npm run build" npm run build
  )
fi

# ── Heavy / external gates (opt-in) ───────────────────────────────
if [[ $FULL -eq 1 ]]; then
  if command -v gitleaks >/dev/null 2>&1; then
    run "gitleaks (tracked)" \
      gitleaks dir . --config .gitleaks.toml --no-banner
  else
    printf '%b⚠ gitleaks binary not found — skipping%b\n\n' "$DIM" "$RESET"
  fi

  if command -v cargo-deny >/dev/null 2>&1; then
    run "cargo-deny" \
      cargo deny check advisories bans licenses sources
  else
    printf '%b⚠ cargo-deny not installed — skipping (cargo install cargo-deny --locked)%b\n\n' "$DIM" "$RESET"
  fi

  if command -v cargo-audit >/dev/null 2>&1; then
    run "cargo-audit" \
      cargo audit
  else
    printf '%b⚠ cargo-audit not installed — skipping (cargo install cargo-audit --locked)%b\n\n' "$DIM" "$RESET"
  fi

  (
    cd ui
    run "ui: npm audit (prod, high+)" \
      npm audit --omit=dev --audit-level=high
  )
fi

printf '%ball gates passed%b\n' "$GREEN" "$RESET"
