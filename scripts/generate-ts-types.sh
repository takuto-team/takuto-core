#!/usr/bin/env bash
set -euo pipefail

# Regenerate the ts-rs-derived dashboard wire DTOs into
# `ui/src/api/generated/`.
#
# The generation itself is performed by `#[cfg(test)]` `export_all_to`
# tests co-located with each DTO (module name `ts_bindings`). Running them
# writes the committed TypeScript mirror. The `ui/src/api/types.ts` barrel
# re-exports these files, so frontend imports stay at `@/api/types`.
#
# CI runs this script then `git diff --exit-code ui/src/api/generated` so a
# Rust DTO change that isn't reflected in committed TypeScript fails the
# build. Locally, run this after changing any `#[derive(TS)]` DTO and commit
# the regenerated files.

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

# The `ts_bindings` filter matches every co-located generation test across
# the workspace (e.g. `…::ts_bindings::export_workflow_dtos`).
cargo test --workspace --locked ts_bindings -- --include-ignored >/dev/null

echo "Generated TypeScript types in ui/src/api/generated/"
