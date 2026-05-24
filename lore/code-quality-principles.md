# Code Quality Principles — project decisions from the 2026-05 audit

This file captures the standing decisions that came out of the May 2026 clean-code audit.
They are **project lore**: future contributors (humans and AI agents) should read them before proposing changes that contradict them, and update this file in the same task if a decision is ever reversed.

Companion: `CODING_STANDARDS.md` (the rules) and `lore/refactor-backlog.md` (the work list).

---

## 1 · We enforce `CODING_STANDARDS.md` §1 file-size rules retroactively

`CODING_STANDARDS.md` §1 sets:

- **Rust:** ~300 LOC of non-test logic per file. Beyond that, split into a `module/` directory with a thin `mod.rs` facade — the `workflow/engine/` pattern is the reference.
- **React:** ~150 LOC per component. Beyond that, extract sub-components / hooks.

These thresholds apply to **existing** code, not only new code. The refactor backlog lists every current violator (`container.rs`, `engine/driver.rs`, `routes/workflows.rs`, `config.rs`, `MyCredentialsSection.tsx`, `TicketDetailModal.tsx`, `AiProviderSettingsSection.tsx`, `Dashboard.tsx`, `api/client.ts`). When you touch one of these files for an unrelated change:

- If your change is small (≤ 20 lines), make the change and leave the file size alone — don't smuggle a split into an unrelated PR (CODING_STANDARDS §5 "minimum viable change").
- If your change is large (a new feature, a significant refactor inside the file), **split the file as part of the same task**.

When adding **new** code, never create a file that ships over the threshold.

---

## 2 · We accept the bundled-runtime-image trade-off

The runtime Docker image bakes:

- Full Rust toolchain (`rustup`, `cargo`, `rustfmt`, `clippy`)
- `build-essential` + system headers
- All four advertised AI provider CLIs (`claude`, `cursor-agent`, `codex`, `opencode`) — even those whose runtime adapters aren't wired yet (Phase 4 codex / opencode are baked because the binary must exist the moment the adapter lands).
- `openvscode-server`
- Playwright Chromium system dependencies

This makes the image larger than a "slim" Rust web service would be. We deliberately accept that cost because:

1. **Zero first-run install latency for advertised features.** Workflows can spawn `claude` / `cursor-agent` / `codex` / `opencode` immediately on a fresh container.
2. **The provisioning + custom-Dockerfile escape hatches already cover admin preferences.** AGENTS.md § "Tool layout and extensibility" documents the three-tier model. The "kitchen sink" is the baked tier — it stays kitchen-sink on purpose.

We **do** document this inline in `Dockerfile` (preamble comment at the runtime stage) and expose `WITH_CODEX` / `WITH_OPENCODE` / `WITH_CURSOR` build args for admins who want a slimmer derivative image. The default image is unchanged.

When you want to add a tool: re-read AGENTS.md § "Tool layout and extensibility" first. The bar for baking a new tool is "Maestro literally fails without it" — most cases belong in `[provisioning]`.

---

## 3 · We migrate to typed `#[from]` error wiring across `MaestroError`

`crates/maestro-core/src/error.rs` had 10 of 13 variants carrying `String` payloads. Forty-one call sites used `.map_err(|e| MaestroError::*(e.to_string()))`, which discards `std::error::Error::source()` — `tracing` and log output never see the root cause.

Going forward:

- New `MaestroError` variants **must** carry a typed payload — either a `#[from] SourceError` for unambiguous one-to-one conversions, or a `#[source] Box<dyn std::error::Error + Send + Sync>` field for variants that intentionally aggregate multiple source types.
- `String`-payload variants are accepted **only** for genuinely-string-shaped errors (e.g. a hand-written domain rejection message — "ticket not found").
- `?` is the default propagation idiom (CODING_STANDARDS §2). Reach for explicit `.map_err(…)` only when you genuinely need to add context.
- The CODING_STANDARDS §2 "Never expose `Box<dyn Error>` in a public API" rule still holds: the boxed source lives **inside** a `MaestroError` variant's `#[source]` field — it is not a public return type. Public functions return `Result<T, MaestroError>` as before.

Logging contract reminder (CODING_STANDARDS §2): **log at the handling site, not the origination site**. Once errors carry their full source chain, the handling site can call `tracing::error!(error = ?e, "context")` and `tracing` walks the chain automatically.

---

## 4 · `Workflow` and `AppState` field visibility is part of the encapsulation contract

`Workflow` (`crates/maestro-core/src/workflow/engine/types.rs`) and `AppState` (`crates/maestro-web/src/state.rs`) both had ~30 `pub` fields. External code mutated `workflow.state = …` directly, bypassing the state machine.

Going forward:

- **Public on `Workflow`:** `id`, `ticket_key`, `state`, `started_at`. Anything else is `pub(crate)` minimum.
- **State transitions go through accessor methods**, never direct assignment. The state machine in `workflow/state.rs` is authoritative; adding a new transition means adding a method, not letting callers write a field.
- **`AppState`** — `pub(crate)` by default; `pub` only at the **true** crate boundary (a field actually read by `maestro-cli` or by an integration test outside the crate).
- **CODING_STANDARDS §2** "`pub(crate)` by default for internal items; `pub` only at true crate boundaries" is the standing rule. Restoring widely-public fields requires an explicit comment justifying why the encapsulation invariant doesn't apply.

This is non-negotiable for `Workflow.state`: any future PR that introduces a `wf.state = …` write outside the state-machine module is a regression.

---

## 5 · CI is the enforcement layer for the §2 quality bar

`CODING_STANDARDS.md` §2 says "`cargo build` must produce **zero warnings** before any commit." Without CI gating, that rule has been aspirational.

Going forward:

- `.github/workflows/` runs, on every PR:
  - `cargo build --workspace --all-targets`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace`
  - `npm --prefix ui ci && npm --prefix ui run build`
- A warning on `main` is a release-blocker, not a future task. If a transient nightly toolchain issue surfaces a warning we can't fix today, **the policy is to pin the toolchain** in `rust-toolchain.toml`, not to relax the gate.
- The release profile (`strip = "symbols"`, `lto = "thin"`, `codegen-units = 1`) is the standing release build configuration; do not override it per-commit.

---

## 6 · Test scaffolding does not ship in the production crate surface

Today `crates/maestro-web/src/lib.rs` exposes `pub mod test_helpers;` without `#[cfg(test)]`. CODING_STANDARDS §2 puts tests in `#[cfg(test)] mod tests` at the bottom of the file they test.

Going forward:

- Test helpers shared across **multiple** test files live behind a `test-utils` cargo feature **or** in a sister `*-testing` crate listed as `[dev-dependencies]`. They never appear in a `pub mod` without a `cfg` gate.
- `.unwrap()` / `.expect()` inside a properly-gated test helper is fine. `.unwrap()` in a `pub mod` that ships to production is a §2 violation regardless of intent.
- New shared test scaffolding must be added behind the feature flag from day one. Promoting test code to production needs an explicit comment justifying it (e.g. "this fixture is the production implementation; the test variant lives at …").

---

## 7 · Module splits

When we split one of the audit's worst-offender files, the cut plan lives in a dated spec under `lore/audits/`. Each spec pins the exact target file list, the per-file LOC budget, the public surface that must remain stable, and an explicit non-goals list — so the split is mechanical and reviewable against the audit's §9 cut plan rather than re-debated PR-by-PR.

- **2026-05-23** — `crates/maestro-core/src/container/runner.rs` (1,513 LOC) split into `mod.rs` + `runner.rs` + `dind_paths.rs` + `volumes.rs` + `secrets_bundle.rs` + `docker_args.rs` + `wrap_command.rs`. Spec: [`lore/audits/2026-05-23-runner-split-spec.md`](audits/2026-05-23-runner-split-spec.md).
- **2026-05-23** — `crates/maestro-core/src/github/auth_resolver.rs` (1,381 LOC) split into `mod.rs` + `resolver.rs` + `decision.rs` + `validator.rs` + `audit.rs` + `errors.rs` under `github/auth_resolver/`. The directory keeps the existing `auth_resolver` module name (not the audit's descriptive `auth/` label) so every `crate::github::auth_resolver::*` import resolves without a re-export shim — same precedent as the runner split. Spec: [`lore/audits/2026-05-23-auth-resolver-split-spec.md`](audits/2026-05-23-auth-resolver-split-spec.md).
- **2026-05-23** — `crates/maestro-core/src/docker_hooks.rs` (1,216 LOC) split into `mod.rs` + `process.rs` + `cursor_auth.rs` + `gh_auth.rs` + `hook_runner.rs` + `status_types.rs` + `status.rs` under `docker_hooks/`. The audit's 10-module cut plan is condensed to 7 because the four provider probes are short `match` arms (not standalone files) and there is no Claude-specific filesystem walker to justify a sibling `claude_auth.rs`. The directory keeps the existing `docker_hooks` module name so every `maestro_core::docker_hooks::*` import (17 call sites across `maestro-web` and `maestro-cli`) resolves without a shim. Spec: [`lore/audits/2026-05-23-docker-hooks-split-spec.md`](audits/2026-05-23-docker-hooks-split-spec.md).

---

## 8 · Encapsulation

When we demote a god-struct's `pub` fields to `pub(crate)` and add typed accessor methods, the cut plan lives in a dated spec under `lore/audits/`. The spec pins the per-field demote/keep decision, the cross-crate call-site count, the accessor naming rule, and an explicit non-goals list — so the change is mechanical and reviewable against the audit's §8 priority list rather than re-debated PR-by-PR. The accessor naming rule is the single source of truth for new fields: callers move from `engine.<field>` to `engine.<field>()` with the same downstream API. `Arc<X>` accessors return `Arc<X>` by clone (preserves `.read().await` and dyn-dispatch chains); `Copy` types return by value; owned non-`Copy` types (`PathBuf`, `Database`) return `&T` by reference so callers `.clone()` only when they need ownership. Adding a `pub` field to one of these structs after the demote is a regression — add an accessor instead.

- **2026-05-24** — `WorkflowEngine` (`crates/maestro-core/src/workflow/engine/mod.rs:47-82`) had 9 `pub` fields; demoted to `pub(crate)` with 9 typed accessor methods. One cross-crate call site (`crates/maestro-web/src/routes/workflows/definitions.rs:28`) was updated from `state.engine.workflows_dir.clone()` to `state.engine.workflows_dir().clone()`; every other read goes through method shims (`workflows_arc()`, `subscribe()`, `event_sender()`) that already existed. `AppState` carving and `WorkflowEngine` sub-struct extraction are deferred phase-2 work. Spec: [`lore/audits/2026-05-24-engine-demote-pubs-spec.md`](audits/2026-05-24-engine-demote-pubs-spec.md).
- **2026-05-24** — `AppState` (`crates/maestro-web/src/state.rs:59-139`) had 20 `pub` fields and 141 bare field reads across `crates/maestro-web/`. Carved into 5 cohesive sub-structs (`EngineState`, `AuthState`, `ConfigState`, `EditorState`, `RunCommandState`) with the audit's prescribed Axum `FromRef` extractor strategy: route handlers and middleware take `State<SubState>` directly for the slices they read, never `State<AppState>`. AppState becomes a 5-field `pub(crate)` composition with a `pub fn new(...)` constructor used by `crates/maestro-cli/src/main.rs` and `test_helpers.rs`. Migration is a 6-commit wave: introduce the carve + rename 141 call sites (commit 1), then migrate handler signatures by route-module wave (auth/admin/sessions/ws → config/jira/onboarding → workflows/* → tickets/credentials/repos + middleware), then lock in with a structural test. Pinning `FromRef` as the single extractor pattern (no `State<AppState>` survives) makes future "needs another slice" handler changes additive — add the extractor — rather than re-debating the encapsulation contract. Spec: [`lore/audits/2026-05-24-appstate-carve-spec.md`](audits/2026-05-24-appstate-carve-spec.md).

---

## 9 · Typed errors

When we carve one of `MaestroError`'s String-payload variants into a typed sub-enum, the cut plan lives in a dated spec under `lore/audits/`. The architecture-binding spec pins (a) the 8 target sub-enums and their module paths, (b) the final `MaestroError` envelope shape, (c) the structural rules sub-enum variants must follow, (d) the per-subsystem deprecation path. Each subsequent per-subsystem spec executes a single migration against those pins — the architecture is not re-debated PR-by-PR. The standing rules from §3 (typed `#[from]` payloads, no `e.to_string()` flattening, log at the handling site with `error = ?e` to walk the chain) are the operating contract; this section's specs are how we get there one subsystem at a time.

The pinned conventions for new sub-enum variants:

- **Foreign error wrapped → `#[from] source`** on a single variant per source type.
- **Operation context → named fields** (`path`, `version`, `user_id`, `column`) — typed identifiers, not free-form sentences.
- **No `format!` inside `#[error("…")]`** — reference fields by name (`{path}`, `{source}`); one line per variant, no terminal punctuation.
- **No `String` payload** on a sub-enum variant. If a variant feels like it needs free-form text, the design is wrong — split it or push the context into named fields.
- **No public accessors** beyond `thiserror` derives. Callers match on variants.

The `MaestroError` envelope target is **transparent `#[from]` per sub-enum** (`MaestroError::Db(#[from] DbError)`, `MaestroError::Jira(#[from] JiraError)`, …), with the existing String variants kept as `#[deprecated]` shims during the migration and removed by a final cleanup PR once every caller is off them. The hybrid alternative (keep `Database(String)` as a permanent peer of `Db(DbError)`) is explicitly rejected — it leaves two ways to express the same failure and defeats the source-chain win that motivates §3.

- **2026-05-24** — db subsystem (`MaestroError::Database(String)`, 18 workspace-wide constructors / 10 inside `crates/maestro-core/src/db/`). First migration; also lands the architecture-binding spec for the next 7 phases (`JiraError`, `GitError`, `GitHubAppError`, `AgentError`, `ClaudeError`, `AuthError`, `ConfigError`). `DbError` lands at `crates/maestro-core/src/db/error.rs` with 6 variants (`Sqlite #[from] rusqlite::Error`, `Migrations`, `DataDir`, `NulByte`, `CommandsJsonEncode`, `CommandsJsonDecode`); `MaestroError` gains `#[error(transparent)] Db(#[from] DbError)`; `impl From<rusqlite::Error> for MaestroError` is rewritten through `DbError::Sqlite` so every `?`-propagation keeps working with a preserved source chain. Migration is 6 atomic commits: land the type + envelope, migrate `mod.rs` / `schema.rs` / `users.rs` / `user_worktree_commands.rs` one file at a time, lock in with a structural test asserting zero `MaestroError::Database(` constructors under `db/`. `MaestroError::Database(String)` stays as a deprecated shim for non-db callers (admin / worktree_commands routes) — removed by the cleanup PR after the AuthError + ConfigError phases. Spec: [`lore/audits/2026-05-24-typed-errors-spec.md`](audits/2026-05-24-typed-errors-spec.md).

---

## When to update this file

Update this file when a **project-level decision** changes — for example:

- We change our mind on the bundled-image trade-off (slim down).
- We adopt or replace `thiserror`.
- We add or remove a CI gate.
- We carve out an explicit exception to a §1 / §2 / §3 rule that future contributors must know about.

Routine refactors that follow these principles do **not** need to touch this file — they update `lore/refactor-backlog.md` instead (or just close the item).
