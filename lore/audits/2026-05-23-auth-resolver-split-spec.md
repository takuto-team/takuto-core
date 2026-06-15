# Refactor spec — split `github/auth_resolver.rs`

Source: 2026-05-21 clean-code audit §9 ("Per-layer cut plans") — second target on the §8 priority #3 list ("Decompose the three Rust god modules"). Current state: `crates/takuto-core/src/github/auth_resolver.rs` = 1,381 LOC (audit §1 worst-offender #5).

## Goal

Split `github/auth_resolver.rs` into focused sub-modules under `github/auth_resolver/` per the audit's cut plan, with zero behaviour change and an identical public surface (every `crate::github::auth_resolver::*` and `takuto_core::github::auth_resolver::*` path keeps resolving).

## Scope (in)

Produce these 6 files under `crates/takuto-core/src/github/auth_resolver/`:

- `mod.rs` — re-exports only (`pub use {decision, errors, validator, audit, resolver}::*;`); ≤ 30 LOC.
- `resolver.rs` — `GitAuthResolver` struct + impl orchestration (`new`, `with_app_token_cwd`, `mode_for_user`, `token_for`, and the `user_has_pat` / `attribute_commits` / `unseal_user_pat` / `materialise_app_token` / `materialise_user_pat` internals); ≤ 300 LOC.
- `decision.rs` — pure `decide_token_source(action, mode, attribute_commits)` + the three enums it operates on (`GitAction` with its `as_str()`, `TokenSource` with its `as_str()`, `GithubAuthMode` with its `Display`). No I/O, no async.
- `validator.rs` — PAT/SSO revalidation via `GhClient`: `revalidate_pat_for_workflow` and `revalidate_sso`. These move as free `pub` fns taking `&GitAuthResolver` (or its `db` handle) and the `GhClient` — they do not stay as methods, to keep `resolver.rs` orchestration-only.
- `audit.rs` — `should_audit_first_use` + the DB audit-row write helper extracted from `materialise_user_pat` (the `touch_last_validated` + `credential_audit::log` pair, lifted to a small `record_first_use(&Database, user_id)` fn).
- `errors.rs` — `GitAuthError` + `GitAuthResult` alias + `auth_warning_payload` + the `SecretToken` (with its redacted `Debug`) and `GitToken` value types.

## Scope (out — non-goals)

- NO behaviour change. The decision matrix in `decide_token_source` is byte-identical; the audit-write call sites fire under the exact same conditions.
- NO renames of public items (`GitAuthResolver`, `GitAuthError`, `GitAuthResult`, `GitAction`, `TokenSource`, `GithubAuthMode`, `SecretToken`, `GitToken`, `decide_token_source`, `auth_warning_payload`).
- NO directory rename: the on-disk dir stays `github/auth_resolver/` (NOT `github/auth/`) so every existing `crate::github::auth_resolver::*` import resolves with zero shim. The audit's "`github/auth/`" naming is treated as descriptive; this spec follows the same precedent as the runner split (keep the existing module path, split inside it).
- NO `TakutoError` changes (that is §8 item #2 — separate task; the resolver still surfaces `GitAuthError` and callers still convert at their boundary).
- NO `ExternalActions` trait changes.
- NO changes to `auth/bundle.rs`, `workflow/engine/*.rs`, `takuto-web/src/state.rs`, or any other caller — every existing import path must continue to resolve unchanged.
- NO modifications to sibling files in `github/` (`mod.rs` line `pub mod auth_resolver;` stays as-is).

## Acceptance criteria

- [ ] Every new file ≤ 300 LOC of non-test code (CODING_STANDARDS §1.1).
- [ ] `resolver.rs` ≤ 300 LOC of non-test code.
- [ ] `cargo build --workspace` produces **zero warnings**.
- [ ] `cargo test --workspace` is green.
- [ ] Public re-export surface of `crate::github::auth_resolver::*` is identical before/after. Verify by grepping `auth_resolver::` across `crates/takuto-core/`, `crates/takuto-web/`, and `crates/takuto-cli/` — every existing path resolves (the 28 references found at spec time).
- [ ] The two existing `thiserror`-derived enums in this file are preserved verbatim: `GitAuthError` (5 variants with their stable `error_code()` mapping) lives in `errors.rs`; any other `thiserror` enum that surfaces here at split time moves with its owning concept and keeps the same variant set and `#[error("…")]` strings.
- [ ] `#[cfg(test)] mod tests` modules stay co-located with the code they cover — the 28-cell decision-matrix tests move into `decision.rs`; the `should_audit_first_use` tests move into `audit.rs`; the resolver integration tests (DB-backed) stay in `resolver.rs`.
- [ ] `SecretToken`'s redacted `Debug` impl is preserved byte-for-byte (the `"bytes: <redacted>"` string is part of the security contract — any logged struct must continue to show `<redacted>`).
- [ ] The `pub mod auth_resolver;` line in `github/mod.rs` is unchanged; no new public re-export at the `github::` level.

## Risks

1. **Async-state plumbing across module boundaries.** `materialise_user_pat` holds a `db.conn().lock()` guard, drops it explicitly before AEAD work, then re-acquires it for the audit write. Extracting the audit write to `audit::record_first_use` must preserve the "drop the guard before `await`-ing CPU-bound or further-locking work" contract (CODING_STANDARDS §2 "Never hold a `RwLock` or `Mutex` guard across an `.await`"). Re-grepping for `.lock().await` inside the new `audit.rs` is the mechanical check.
2. **`revalidate_pat_for_workflow` writes a `credential_audit` row on failure** before mapping to `GitAuthError`. Moving it to `validator.rs` as a free fn means the `&Database` handle reaches the audit-write inside the same call; do not duplicate the `code` → `error_code` mapping (it must remain a single match arm, not split between `validator.rs` and `audit.rs`).
3. **`SecretToken` visibility creep.** It is `pub` today and used outside the resolver only as a field of `GitToken`. After the split it stays `pub` in `errors.rs` (because `GitToken.bearer: SecretToken` is part of the public API), but no new `pub` accessor is added — the `expose(&self) -> &str` method is the only escape hatch and remains the only one.
4. **`auth_warning_payload` callers in `workflow/engine/*`.** Two call sites (`persistence.rs:296`, `transitions.rs:272`) use `crate::github::auth_resolver::auth_warning_payload(&e)`. The fn moves into `errors.rs` but must remain re-exported at `auth_resolver::auth_warning_payload` via `mod.rs` — no caller-side import edits are permitted by this spec.
5. **Test-fixture imports (`in_mem_db_with_master_key`, `seed_user`, etc.) reach private items via `super::`.** When tests move out alongside the production code, those fixtures must move with them or be re-exposed `pub(super)` from a small `test_support` mod inside `auth_resolver/` — do not concentrate all tests in one sub-module.
