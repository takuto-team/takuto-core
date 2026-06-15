# Refactor spec — demote `WorkflowEngine` pub fields

Source: 2026-05-21 clean-code audit §8 #1 ("pub-field god structs"), **phase 1 of 2**. Phase 2 — carving `AppState` and/or `WorkflowEngine` into sub-structs — is deferred to a follow-up team and is an explicit non-goal here.

## Goal

Demote `WorkflowEngine`'s 9 `pub` fields (`crates/takuto-core/src/workflow/engine/mod.rs:47-82`) to `pub(crate)` and expose typed accessor methods so the struct stops leaking mutable internals. Zero behaviour change; the only call-site shape change is one cross-crate read at `crates/takuto-web/src/routes/workflows/definitions.rs:28`.

## Scope (in)

12 fields are in scope. The current state and decision per field:

| # | Field | Current vis | Decision | Accessor |
|---|-------|-------------|----------|----------|
| 1 | `config: Arc<RwLock<Config>>` | `pub` | **demote → `pub(crate)`** | `pub fn config(&self) -> Arc<RwLock<Config>>` (clone) |
| 2 | `repository: Arc<WorkflowRepository>` | `pub(crate)` | **keep `pub(crate)`** (no-op; already correct) | none |
| 3 | `event_bus: Arc<WorkflowEventBus>` | `pub(crate)` | **keep `pub(crate)`** (no-op) | none |
| 4 | `actions: Arc<dyn ExternalActions>` | `pub` | **demote → `pub(crate)`** | `pub fn actions(&self) -> Arc<dyn ExternalActions>` (clone) |
| 5 | `agent_run_semaphore: Arc<Semaphore>` | private | **keep private** (no-op) | none |
| 6 | `suppress_cancelled_as_error: Arc<AtomicBool>` | `pub` | **demote → `pub(crate)`** | `pub fn suppress_cancelled_as_error(&self) -> Arc<AtomicBool>` (clone) |
| 7 | `jira_available: Arc<AtomicBool>` | `pub` | **demote → `pub(crate)`** | `pub fn jira_available(&self) -> Arc<AtomicBool>` (clone) |
| 8 | `ticketing_system: TicketingSystem` | `pub` | **demote → `pub(crate)`** | `pub fn ticketing_system(&self) -> TicketingSystem` (Copy by value) |
| 9 | `workflows_dir: PathBuf` | `pub` | **demote → `pub(crate)`** | `pub fn workflows_dir(&self) -> &Path` (reference) |
| 10 | `db: Option<Database>` | `pub` | **demote → `pub(crate)`** | `pub fn db(&self) -> Option<&Database>` (reference) |
| 11 | `git_auth_resolver: Option<Arc<GitAuthResolver>>` | `pub` | **demote → `pub(crate)`** | `pub fn git_auth_resolver(&self) -> Option<Arc<GitAuthResolver>>` (clone) |
| 12 | `gh_client: Arc<dyn GhClient>` | `pub` | **demote → `pub(crate)`** | `pub fn gh_client(&self) -> Arc<dyn GhClient>` (clone) |

Service structs (`persistence`, `lifecycle`, `transitions`, `definitions`) are already private — out of scope.

## Cross-crate call-site count

Greps across `crates/takuto-cli/`, `crates/takuto-web/`, and `crates/takuto-core/` (excl. `workflow/engine/`) for `engine.<field>` / `state.engine.<field>`:

| Field | Cross-crate call sites |
|-------|------------------------|
| `workflows_dir` | **1** — `crates/takuto-web/src/routes/workflows/definitions.rs:28` (`state.engine.workflows_dir.clone()` → `state.engine.workflows_dir().clone()`) |
| all 11 other candidates | **0** |

**Total cross-crate field reads: 1.** Every other field is either consumed inside `workflow/engine/*` (where `pub(crate)` keeps it accessible) or read via existing method shims (`workflows_arc()`, `subscribe()`, `event_sender()`, `event_subscriber_count()`, `broadcast_event()`). `AppState` holds its own independent `config` / `db` / `jira_available` / `ticketing_system` / `git_auth_resolver` / `gh_client` fields (`crates/takuto-web/src/state.rs:60-114`); the engine duplicates are not re-read from outside the crate.

## Accessor naming convention (pinned)

The accessor takes the field's identifier verbatim and returns:

- `Arc<X>` → `Arc<X>` by `.clone()`. Preserves the existing `.read().await` (`RwLock`) and dyn-dispatch (`ExternalActions`, `GhClient`) chains at call sites with one extra `()`.
- `Option<Arc<X>>` → `Option<Arc<X>>` by clone (same logic).
- `Copy` value types (`TicketingSystem`) → by value.
- Owned non-`Copy` values (`PathBuf`, `Database`) → by **reference** (`&Path`, `Option<&Database>`); callers `.clone()` explicitly when ownership is needed (the one `workflows_dir` site already does).

Rule of thumb: **call sites change from `engine.<field>` to `engine.<field>()`** and otherwise stay byte-identical. No new wrapper types; no rename; no `_arc` / `get_` prefix.

## Acceptance criteria

- [ ] `cargo build --workspace` produces **zero warnings**.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` is green.
- [ ] `cargo test --workspace` matches the pre-change baseline (no test count or pass/fail delta).
- [ ] The `WorkflowEngine` struct definition (`workflow/engine/mod.rs:47-82`) has **zero `pub <field>:` lines**. Every field is `pub(crate)` or private. (Verifiable with `grep -E "^    pub [a-z_]+:" crates/takuto-core/src/workflow/engine/mod.rs` returning empty.)
- [ ] The 9 new accessors exist with the signatures in the table above; each is one line (`self.<field>.clone()` / `&self.<field>` / `self.<field>`). No accessor does work other than return.
- [ ] The single cross-crate call site at `definitions.rs:28` is updated to `state.engine.workflows_dir().clone()`. No other caller-side edits.
- [ ] No field type changes. No new fields. No method renames. No changes to `new` / `new_with_db` / `with_gh_client` / `with_git_auth_resolver` signatures.
- [ ] No `AppState` changes. No service-struct (persistence / lifecycle / transitions / definitions) changes.

## Risks

1. **Internal call-site churn inside `workflow/engine/*`.** `pub(crate)` keeps direct field access legal from sibling modules (`driver.rs`, `step_runner.rs`, etc.), so internal `self.<field>` and `engine.<field>` reads from within the crate compile unchanged. Mechanical check: `grep -rn "engine\.\(config\|actions\|jira_available\|ticketing_system\|workflows_dir\|db\|git_auth_resolver\|gh_client\|suppress_cancelled_as_error\)" crates/takuto-core/` should still resolve after the demote.
2. **Test fixtures in `mod.rs` tests.** `mod.rs:1193-1216` reads `engine.ticketing_system`, `engine.jira_available`, `engine.workflows_dir` directly from inside the same module — these stay legal under `pub(crate)` and need no edits. Tests living in sibling crates (`crates/takuto-web/tests/*.rs`) currently use only method shims (`workflows_arc()`, `subscribe()`, `event_sender()`), so they are unaffected.
3. **Future drift.** Demotion makes "add a new `pub` field" a deliberate choice, but the next contributor who needs to expose state from the engine to a route must add an accessor — not flip the field back to `pub`. CODING_STANDARDS §2 "`pub(crate)` by default" plus this spec's pinned accessor rule are the standing guard.
4. **Accessor return-type contract.** Returning `Arc<X>` by clone bumps the Arc refcount once per call; for the hot path (`event_sender()` style reads inside loops) this is the same cost as today's implicit `Arc::clone(&engine.field)`. Returning `&Path` / `Option<&Database>` for owned fields avoids unnecessary clones — the one cross-crate `workflows_dir` site explicitly opts in via `.clone()`.

## Non-goals (explicit)

- **NO** `AppState` changes (deferred to next team — separate spec).
- **NO** carving of `WorkflowEngine` into sub-structs (also deferred).
- **NO** changes to service structs (`persistence`, `lifecycle`, `transitions`, `definitions`).
- **NO** `TakutoError`, `ExternalActions` trait, or serde-shape changes (§8 #2 / §8 #4 are separate tasks).
- **NO** new fields, no method renames, no signature changes on the four existing constructor / builder fns.
- **NO** changes to `pub use driver::resolve_worktree_init_commands;` or `pub use types::{...};` re-exports.
