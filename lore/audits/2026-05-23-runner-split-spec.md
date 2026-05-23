# Refactor spec — split `container/runner.rs`

Source: 2026-05-21 clean-code audit §9 ("Per-layer cut plans") — first target on the §8 priority #3 list ("Decompose the three Rust god modules"). Current state: `crates/maestro-core/src/container/runner.rs` = 1,513 LOC (audit §1 worst-offender #4).

## Goal

Split `container/runner.rs` into focused sub-modules under `container/` per the audit's cut plan, with zero behaviour change and an identical public surface.

## Scope (in)

Produce these 7 files under `crates/maestro-core/src/container/`:

- `mod.rs` — re-exports only; ≤ 30 LOC.
- `runner.rs` — `ContainerRunner` struct + impl orchestration (`new`, `with_*`, `is_available`, `next_container_name`, `wrap_command`, `wrap_shell_command`, `force_remove_all`, `cleanup_for_ticket`, `discover_worker_image`, `provider_extra_args`, `has_secrets_bundle`); ≤ 250 LOC.
- `dind_paths.rs` — `translate_path_for_dind`, `translate_path_for_dind_inner`, `is_remote_docker_daemon`, plus the `DIND_DATA_PREFIX_ENV` / `MAESTRO_DATA_DIR_HOST_PREFIX` consts.
- `volumes.rs` — `build_volume_args` (isolated vs legacy workspaces) and the `WORKER_VOLUMES` const.
- `secrets_bundle.rs` — `apply_secrets_bundle_to_args`, `passthrough_is_bundled`.
- `docker_args.rs` — `base_docker_args` (lifted to a free `pub(crate) fn` taking what it needs) and the `WORKER_ENV` / `PASSTHROUGH_ENV` consts (env curation lives here).
- `wrap_command.rs` — `wrap_command` body plus the 4 shell snippets (`restore`, `fix_perms`, `gh_token`, `bundle_source` aka `BUNDLE_SOURCING_SH`) as `const &str` blocks at the top of the file.

## Scope (out — non-goals)

- NO behaviour change. Byte-for-byte identical `docker run` argv for every code path.
- NO renames of public items (`ContainerRunner`, `wrap_command`, `wrap_shell_command`, `translate_path_for_dind`, `build_volume_args`, `passthrough_is_bundled`, `apply_secrets_bundle_to_args`, `WORKER_ENV`, `PASSTHROUGH_ENV`, `WORKER_VOLUMES`, `BUNDLE_SOURCING_SH`).
- NO changes to the `ExternalActions` trait surface (audit AGENTS.md "External actions boundary").
- NO `MaestroError` rework (that is §8 item #2 — separate task).
- NO `ContainerRunner` field reshuffling / encapsulation rework (that is §8 item #1).
- NO modifications to sibling files in `container/` (`editor.rs`, `port_scanner.rs`, `reap.rs`, `run_command.rs`, `terminal.rs`).

## Acceptance criteria

- [ ] Every new file ≤ 300 LOC of non-test code (CODING_STANDARDS §1.1).
- [ ] `runner.rs` ≤ 250 LOC of non-test code.
- [ ] `cargo build --workspace` produces **zero warnings**.
- [ ] `cargo test --workspace` is green.
- [ ] Public re-export surface of `crate::container::*` is identical before/after. Verify by grepping for `container::` and `super::` references across `crates/maestro-cli/`, `crates/maestro-web/`, and the rest of `crates/maestro-core/` — every existing path resolves.
- [ ] The hardcoded `SECRET_PASSTHROUGH` list (currently `runner.rs:480`) stays a single source of truth. If it moves, it moves to one place (likely `secrets_bundle.rs`); do not duplicate.
- [ ] The 4 inline shell snippets used by `wrap_command` (`restore` / `fix_perms` / `gh_token` / `bundle_source`) end up in `wrap_command.rs` as `const &str` blocks at the top of the file (NOT a separate "shell snippets" module). The audit's "no inline bash strings" guidance means "lifted to named consts at file top", not "moved to their own crate".
- [ ] `#[cfg(test)] mod tests` modules stay co-located with the code they cover — tests for `translate_path_for_dind*` move with it into `dind_paths.rs`; bundle-sourcing tests move with `BUNDLE_SOURCING_SH` into `wrap_command.rs`; etc. Do not concentrate all tests into `runner.rs`.

## Risks

1. **Test fixture imports break.** Tests reach into private helpers (`has_env`, `has_volume`, `flag_value`, `runner()`, `isolated_runner()`, `legacy_runner()`) via `super::`. When tests move, those helpers must move with them or be re-exposed `pub(super)` from a shared `test_support` mod.
2. **Hidden cross-fn private usage.** `base_docker_args` is a private method on `ContainerRunner` today — lifting it to a free function in `docker_args.rs` means it must take `&ContainerRunner` (or the small set of fields it reads) as arguments without leaking new public accessors.
3. **Borrow / lifetime regressions across module boundaries.** Helpers that currently take `&self` and return `Vec<String>` are safe to lift, but the secrets-bundle path uses `Arc<WorkerSecretsBundle>` and reads `bundle.host_dir()` / `bundle.extra_env` — keep those reads inside the secrets-bundle helper to avoid threading a second borrow through `base_docker_args`.
4. **Const visibility creep.** `WORKER_ENV` / `PASSTHROUGH_ENV` / `WORKER_VOLUMES` are `pub(crate)` today; keep them `pub(crate)` in their new home — do not widen to `pub`.
