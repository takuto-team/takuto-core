# Refactor spec — carve `AppState` into focused sub-structs

Source: 2026-05-21 clean-code audit §8 #1 ("pub-field god structs"), **phase 2 of 2**. Phase 1 (`WorkflowEngine` pub-field demote) shipped on 2026-05-24 — see [`2026-05-24-engine-demote-pubs-spec.md`](2026-05-24-engine-demote-pubs-spec.md). This phase carves `crates/takuto-web/src/state.rs` (`AppState`, 20 `pub` fields) into 5 cohesive sub-structs so route handlers extract only the slice they read via Axum's `FromRef`.

## Goal

Replace `AppState`'s flat 20-field shape with a 5-field composition (`AppState { engine, auth, config, editor, run_command }`). Route handlers move from `State<AppState>` to `State<EngineState>` / `State<AuthState>` / … extracted via `FromRef`. Zero behaviour change; every external HTTP/WS contract is byte-identical.

## Per-field assignment

20 fields → 5 sub-structs. Each sub-struct is `pub` + `#[derive(Clone)]` (cheap — every field is `Arc<_>`, `Copy`, `PathBuf`, or `Option<…>`). All sub-struct fields are `pub` (read by handlers via the extractor).

| Sub-struct | Fields | Why this grouping |
|------------|--------|-------------------|
| `EngineState` (4) | `engine`, `polling_paused`, `clone_in_progress`, `system_status` | Live mutable handles for long-running background work: the workflow engine + the two `AtomicBool` gates (poller pause, in-flight repo-clone) + the integration health snapshot that `PUT /api/config/agent` mutates at runtime. Used by `ws.rs`, `workflows/*`, `tickets.rs`, `repositories.rs`, `auth.rs:75`, `config_agent.rs`, `onboarding.rs`. |
| `AuthState` (3) | `db`, `gh_client`, `git_auth_resolver` | The DB + the two GitHub-auth shims. Every protected route's middleware reads `db`; `gh_client` and `git_auth_resolver` are read by PAT/credentials handlers (`credentials.rs:605`, `tickets.rs:194`, `repos.rs:249`). |
| `ConfigState` (6) | `config`, `config_path`, `config_writer`, `ticketing_system`, `jira_available`, `preflight_error` | The live `Config` + how to persist it + boot-time integration flags read out of `[general]` + the Phase-0 deprecated preflight string. Used by `config.rs`, `config_agent.rs`, `jira.rs`, `credentials.rs:263`, `tickets.rs`, `workflows/*`, both middleware layers. |
| `EditorState` (5) | `editor_scanners`, `dynamic_forwards`, `terminal_ports`, `editor_bundles`, `path_token_registry` | Editor session container state — all 5 are keyed by `ticket_key` and registered/cleared by the same lifecycle (open editor / close editor). `path_token_registry` belongs here because `sessions::proxy_session` resolves the token → editor backend on every `/s/{token}/…` request. |
| `RunCommandState` (2) | `run_commands`, `run_command_bundles` | Run-command companion to `EditorState`. Two fields are intentional — adding a third (e.g. a future `run_command_scanners` map) is the natural extension point. |

**Total bare field reads** (`state.<field>` grep across `crates/takuto-web/src/`): **141** call sites (excluding method calls `.clone() / .is_active() / .is_some() / .is_none() / .get() / .read() / .write()`). Top 5: `db` (41), `engine` (34), `config` (31), `path_token_registry` (10), `run_commands` (8).

## Axum extractor strategy (pinned)

**Pure `FromRef`** — AppState manually impls `axum::extract::FromRef<AppState>` for each of the 5 sub-structs (4-line `from_ref` returning `self.<field>.clone()`). Route handlers take **one or more** `State<SubState>` extractors per slice they read; Axum invokes `FromRef::from_ref(&app_state)` for each. No `State<AppState>` in handlers after migration.

**Rejected alternatives:**
- *Accessor methods on AppState* (`state.engine_state() -> &EngineState`) — keeps `State<AppState>` in every handler signature, defeats the encapsulation point of the carve. Handler signatures should document which slice is read.
- *Hybrid (FromRef + State<AppState>)* — produces two patterns side-by-side; new handlers ambiguous about which to pick. Rejected for consistency.

**Exception**: middleware (`middleware/csrf.rs`, `middleware/security_headers.rs`) extracts `State<ConfigState>` — same `FromRef` mechanism, no `State<AppState>` survives.

**Construction**: `AppState` exposes `pub fn new(engine: EngineState, auth: AuthState, config: ConfigState, editor: EditorState, run_command: RunCommandState) -> Self`. `crates/takuto-cli/src/main.rs:1053` and `crates/takuto-web/src/test_helpers.rs:75` build the 5 sub-structs first, then call `AppState::new(...)`. AppState's 5 fields are `pub(crate)` (no cross-crate struct-literal construction); sub-structs are `pub` with `pub` fields.

## Migration plan (6 commits)

1. **Carve + compose** (1 commit, biggest). Define `EngineState`, `AuthState`, `ConfigState`, `EditorState`, `RunCommandState` in `state.rs`. Re-shape `AppState` to a 5-field composition. Add `FromRef<AppState>` impls. Add `AppState::new(...)`. Rewrite `crates/takuto-cli/src/main.rs:1053` and `test_helpers.rs:75` to build sub-structs first. Rename all 141 call sites from `state.<field>` to `state.<sub>.<field>` (e.g. `state.engine` → `state.engine.engine`, `state.db` → `state.auth.db`). Handler signatures still take `State<AppState>` — no extractor changes yet. `cargo test --workspace` matches baseline (1025/0/1).
2. **Wave A — auth/admin/sessions/ws** (~52 call sites). Migrate handler signatures from `State<AppState>` to `State<AuthState>` / `State<EngineState>` / `State<EditorState>` as needed. Drop `state.auth.` / `state.engine.` prefixes — accessed directly on the extracted sub-state.
3. **Wave B — config/config_agent/onboarding/jira** (~30 call sites). Same pattern: extract `State<ConfigState>` / `State<EngineState>` / `State<AuthState>`.
4. **Wave C — workflows/{list,editor,run_commands,manual,lifecycle,definitions,port_tracking}** (~50 call sites). The largest wave: editor + run-command handlers commonly need 3-4 slices.
5. **Wave D — tickets/credentials/repos/repositories/github/polling/worktree_commands** (~30 call sites) + middleware (csrf, security_headers — ~3 call sites). Final handlers + middleware extractor swap.
6. **Lock in** (1 commit). Verify zero `State<AppState>` extractors remain (grep gate). Demote `AppState`'s 5 sub-struct fields to `pub(crate)` if not already. Add the structural test guard in `crates/takuto-web/tests/` matching the engine-demote precedent. Update `AGENTS.md` if it documents the AppState shape.

Each wave is one atomic commit, must compile + `cargo test --workspace` is green.

## Acceptance criteria

- [ ] `cargo build --workspace` produces **zero warnings**; `cargo clippy --workspace --all-targets -- -D warnings` is green.
- [ ] `cargo test --workspace` matches the pre-change baseline (1025 pass / 0 fail / 1 ignored — no count or pass/fail delta).
- [ ] `AppState` struct definition has **exactly 5 fields**, all `pub(crate)` (`engine: EngineState`, `auth: AuthState`, `config: ConfigState`, `editor: EditorState`, `run_command: RunCommandState`). Verifiable: `grep -E "^    pub(\(crate\))? [a-z_]+:" crates/takuto-web/src/state.rs` returns **only the 5 composition fields + the 20 sub-struct fields**.
- [ ] Zero `State<AppState>` in route handler or middleware signatures (verifiable: `grep -rn "State<AppState>" crates/takuto-web/src/routes/ crates/takuto-web/src/middleware/` returns empty).
- [ ] Every external HTTP route + WS handler returns byte-identical responses for the integration test suite — no behaviour change, no serde-shape change, no contract change.
- [ ] `crates/takuto-cli/src/main.rs:1053` constructs AppState via `AppState::new(...)` only (no struct literal). `test_helpers.rs` same.
- [ ] FromRef impls are **single-purpose**: each `from_ref` returns `self.<field>.clone()` — no field projection, no synthesis, no derived state.

## Risks (high blast radius)

1. **WebSocket handler (`routes/ws.rs`)**. `ws_handler` takes `State(state): State<AppState>`, reads `state.db` for session validation, then passes the whole `AppState` into `handle_socket(socket, state, viewer_user_id)` which holds it for the connection lifetime and reads `state.engine`. After the carve, `ws_handler` extracts `State<AuthState>` + `State<EngineState>` and `handle_socket` takes only `EngineState` (the auth slice is consumed at upgrade time, not held). This is a real signature change inside the WS module — surface it explicitly in Wave A.
2. **Test fixtures (`crates/takuto-web/src/test_helpers.rs:46-97`)** build a mock `AppState` with all 20 fields inline. Migration step 1 must atomically rewrite this fixture or every `tests/*.rs` integration test breaks. The fixture stays a single fn; it constructs 5 sub-structs and calls `AppState::new(...)`. No test file edits required (they call `test_state_with_db()` opaquely).
3. **`AppState: Clone` propagation.** AppState derives `Clone` today; the 5 sub-structs must also derive `Clone` for FromRef's `.clone()` to compile. Every sub-struct field is `Arc<_>` / `Copy` / `Option<Arc<_>>` / `PathBuf` — all `Clone`. Verified field-by-field. No `#[derive(Clone)]` boilerplate risk.
4. **Background tasks hold their own clones.** `snapshot_task`, `config_watcher`, `pr_merge_poller`, `JiraPoller`, `GitHubPoller` are spawned in `crates/takuto-cli/src/main.rs:1100-1130` with typed handles (`engine.clone()`, `config.clone()`, `polling_paused.clone()`) — not `AppState`. They are unaffected. **Mechanical check after Wave A**: `grep -rn "AppState\b" crates/takuto-cli/` returns only the `main.rs:1053` construction site.
5. **Middleware `from_fn_with_state`.** `server.rs:328/336/375` registers middleware with `state.clone()` — the cloned thing is the full `AppState`, and Axum then routes `FromRef` extraction for each middleware. This works unchanged with the carved AppState (FromRef is the whole point); `csrf_middleware` and `security_headers_middleware` extract `State<ConfigState>` directly.
6. **Field-rename churn during step 1.** 141 call sites change shape (`state.engine` → `state.engine.engine`). This is mechanical but easy to miscount — the per-field grep counts above are the verification anchor. The biggest cluster is `state.db` (41) → `state.auth.db`.

## Non-goals (explicit)

- **NO** logic changes inside route handlers. Only signature edits and field-access path renames.
- **NO** changes to `WorkflowEngine`, `TakutoError`, `ExternalActions`, serde-shapes of any HTTP/WS contract, or the `Database` type.
- **NO** new fields, no field renames, no field type changes. The 20 fields keep their names, types, and docs (the docs migrate with the fields).
- **NO** test rewrites — only the mechanical `state.<field>` → `state.<sub>.<field>` (step 1) then `sub_state.<field>` (waves) updates as signatures change.
- **NO** new constructors on sub-structs (callers build them with struct literals — same pattern as `DynamicPortForward` today). A future task may add `pub fn new(...)` if a sub-struct gains an invariant; this spec does not.
- **NO** `WorkflowEngine` re-carving (its 9 fields stayed flat by design — phase-1 spec).
