# Refactor spec — split `docker_hooks.rs`

Source: 2026-05-21 clean-code audit §9 ("Per-layer cut plans") — third target on the §8 priority #3 list ("Decompose the three Rust god modules"). Current state: `crates/takuto-core/src/docker_hooks.rs` = 1,216 LOC (audit §1 worst-offender #6).

## Goal

Split `docker_hooks.rs` into focused sub-modules under `docker_hooks/` per the audit's cut plan, with zero behaviour change and an identical public surface (every `crate::docker_hooks::*` and `takuto_core::docker_hooks::*` path keeps resolving).

## Scope (in)

Produce these 7 files under `crates/takuto-core/src/docker_hooks/`:

- `mod.rs` — re-exports only (`pub use {hook_runner, process, cursor_auth, gh_auth, status_types, status}::*;`); ≤ 30 LOC.
- `process.rs` — process-spawn primitives shared by every probe: `auth_cmd_ok` (timeout-bounded probe runner), `configure_auth_command_unix` (both `cfg(unix)` and `cfg(not(unix))` arms), `kill_process_group_best_effort` (both arms), `preflight_home`. Today at lines 21-25 + 169-199 + 256-290.
- `cursor_auth.rs` — Cursor on-disk auth heuristics: `cursor_agent_auth_likely_on_disk`, `cursor_data_tree_looks_populated`, `json_config_suggests_auth`, `json_value_has_auth_fields`. Today at lines 27-167. Co-located `cursor_preflight_tests` mod (today lines 886-912) moves here.
- `gh_auth.rs` — `gh_auth_recover_expired_token` (the `gh auth switch` recovery for expired App-installation tokens). Today at lines 738-842.
- `hook_runner.rs` — `run_hook_commands` (config-driven `bash -c` hook executor). Today at lines 201-254.
- `status_types.rs` — the `SystemStatus` shape and its siblings: `SystemStatus`, `GitHubStatus`, `ProviderStatus`, `TicketingStatus`, `StructuredWarning` (incl. `critical` / `warning` ctors and the `pub fn info` escape hatch used by `takuto-web::config_agent`), `PreflightResult`, `impl SystemStatus { has_critical }`, `impl Default for SystemStatus`. Today at lines 292-297 + 299-448.
- `status.rs` — collection + writability + acli + the deprecated `preflight()`: `collect_system_status`, `collect_system_status_with_db` (GitHub / Ticketing / Provider branches stay inline as today), `check_config_dir_writable`, `check_acli_auth`, and the `#[deprecated]` `preflight(&Config)` shim. Today at lines 450-729 + 731-736 + 854-884. Co-located `system_status_tests` mod (today lines 914-1216) moves here.

## Scope (out — non-goals)

- NO behaviour change. The structured-warning emission order, severities, codes, and messages are byte-identical; `collect_system_status_with_db` writes the same warnings to the same `Vec<StructuredWarning>` in the same sequence.
- NO renames of public items (`run_hook_commands`, `PreflightResult`, `SystemStatus`, `GitHubStatus`, `ProviderStatus`, `TicketingStatus`, `StructuredWarning`, `collect_system_status`, `collect_system_status_with_db`, `check_config_dir_writable`, `check_acli_auth`, `preflight`).
- NO reduction in module count below 7. The audit's 10-module cut plan is condensed because (a) the Claude probe is a single inline `auth_cmd_ok("claude", ["auth", "status"])` call — there is no claude-specific filesystem walker, so no `claude.rs`; (b) the four provider branches (claude / cursor / codex / opencode) are short `match` arms in `collect_system_status_with_db` — splitting each into its own file produces sub-30-LOC stubs and obscures the shared decision flow; (c) "config schema" lives in `config.rs` already, leaving only `check_config_dir_writable` for a hypothetical `schema.rs` — that single fn sits next to `collect_system_status` instead; (d) `check_acli_auth` is one line, kept beside its only caller (`collect_system_status_with_db`).
- NO directory rename: on-disk dir stays `docker_hooks/` (NOT `preflight/` or `auth_detection/`) so every existing `crate::docker_hooks::*` import resolves with zero shim — same precedent as the runner and auth_resolver splits.
- NO `TakutoError` changes (that is §8 item #2 — separate task).
- NO `ExternalActions` trait changes.
- NO changes to `takuto-web/src/state.rs`, `takuto-cli/src/main.rs`, or any other caller — every existing `docker_hooks::*` import path must continue to resolve unchanged.
- NO modification to the `pub mod docker_hooks;` line in `crates/takuto-core/src/lib.rs:25`.

## Acceptance criteria

- [ ] Every new file ≤ 300 LOC of non-test code (CODING_STANDARDS §1.1).
- [ ] `cargo build --workspace` produces **zero warnings**.
- [ ] `cargo test --workspace` is green.
- [ ] Public re-export surface of `crate::docker_hooks::*` is identical before/after. Verify by grepping `docker_hooks::` across `crates/takuto-core/`, `crates/takuto-web/`, and `crates/takuto-cli/` — every existing path resolves (the 17 references found at spec time, covering `SystemStatus`, `StructuredWarning`, `run_hook_commands`, `collect_system_status`, `collect_system_status_with_db`, `check_config_dir_writable`).
- [ ] `SystemStatus`, `GitHubStatus`, `ProviderStatus`, `TicketingStatus`, `StructuredWarning`, `PreflightResult` are preserved verbatim — same field names, same `#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]`, same `Default for SystemStatus` body. The `serde` representation is part of the HTTP contract for `GET /api/onboarding/status`; field renames are a regression.
- [ ] The hardcoded warning codes (`gh_auth_missing`, `acli_not_authenticated`, `claude_not_authenticated`, `cursor_not_authenticated`, `cursor_cli_missing`, `master_key_unavailable`, `secret_key_world_readable`, `config_dir_not_writable`) stay byte-identical strings — the UI switches on `code` for localised copy.
- [ ] `StructuredWarning::info` stays `pub` (used by `takuto-web/src/routes/config_agent.rs:330` outside the crate). `critical` / `warning` stay crate-private as today.
- [ ] `#[cfg(test)] mod tests` modules stay co-located with the code they cover — `cursor_preflight_tests` moves into `cursor_auth.rs`; `system_status_tests` moves into `status.rs` (it covers `collect_system_status` + `check_config_dir_writable`). The `ENV_LOCK: Mutex<()>` and `isolate_env()` fixture move with `system_status_tests`. Do not concentrate all tests in `mod.rs`.
- [ ] No `TakutoError` variant changes, no new `ExternalActions` trait methods. The deprecated `preflight()` still returns `Err(TakutoError::Config(_))` via the same `status.has_critical()` branch.

## Risks

1. **Process-env serialisation in tests.** `system_status_tests` mutates `HOME` / `CLAUDE_CODE_OAUTH_TOKEN` / `CURSOR_API_KEY` / `ANTHROPIC_BASE_URL` / `CURSOR_CONFIG_DIR` / `XDG_CONFIG_HOME` under a process-wide `Mutex<()>`. When the module moves, the `ENV_LOCK` and `isolate_env` helpers must move with the tests — promoting either to `pub(super)` in a shared `test_support` mod is acceptable, but **do not** split env-mutating tests across files (the lock only serialises within one mod).
2. **`check_config_dir_writable` placement.** It is logically a "config dir check" but its only caller is `takuto-web/src/routes/config_agent.rs:313` and its co-located tests live in `system_status_tests`. Putting it next to `collect_system_status` (in `status.rs`) keeps the tests undisrupted; a sibling `config_writable.rs` would split the tests away from the fn and add a fourth small file.
3. **`PreflightResult` placement.** It is the return type of the deprecated `preflight()` but it is a type, not a function. Keeping it in `status_types.rs` (with the other public types) and the function in `status.rs` follows the auth_resolver precedent (types in `errors.rs`, behaviour in `resolver.rs`).
4. **`gh_auth_recover_expired_token` is private today.** Lifting it to its own file means keeping it `pub(super)` (or `pub(crate)`) so `collect_system_status_with_db` can call it across module boundaries — do not widen to `pub`. The fragile line-based YAML parser inside it does not get rewritten as part of this split (the `TODO` comment stays where it is).
