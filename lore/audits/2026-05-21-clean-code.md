# Maestro — Clean Code Audit (2026-05-21)

Audit performed at commit `afc09e2`. Findings span Rust backend, React/TypeScript frontend, and Docker layer. Companion to `lore/refactor-backlog.md`.

## Overall grade: B−

The Rust workspace is well-typed at the error boundary and almost free of production `unwrap`/`expect` (22 in production paths, all in `src/test_helpers.rs`, `src/db/tests_phase2a_master_key.rs`, or infallible HTTP builders) but it has accumulated five Rust files over 1,000 LOC and one struct with 26 fields. The React/TypeScript dashboard is the strongest of the three layers (strict TS, zero `any`, zero snapshot tests) but it has four files over 400 LOC with too much orchestration logic. The Dockerfile is a 362-line, 29-RUN bare-metal toolchain image with no digest-pinned base layers and no `USER` directive.

## Scale

| Metric | Backend (Rust) | Frontend (React/TS) | Docker |
|---|---:|---:|---:|
| Source files | 140 `*.rs` | 60 components/pages + 19 hooks/utils + 10 api | 1 Dockerfile, 2 compose files |
| Total LOC | 59,645 | ~16,279 | 548 |
| Files > 400 LOC | 51 | 4 | 1 |
| Largest file | `config.rs` 2,693 | `MyCredentialsSection.test.tsx` 632 | `Dockerfile` 362 |
| `unwrap()` in prod paths | 22 (mostly infallible HTTP builders) | n/a | n/a |
| `expect()` in prod paths | 28 | n/a | n/a |
| `.clone()` (prod) | 793 | n/a | n/a |
| `tokio::spawn` fire-and-forget | 21 of 34 | n/a | n/a |
| `Result<_, String>` signatures | 33 | n/a | n/a |
| `any` / `@ts-ignore` | n/a | 0 / 0 | n/a |
| Snapshot tests / test-IDs | n/a | 0 / 0 | n/a |
| RUN layers (runtime stage) | n/a | n/a | 26 |

## 12 worst offenders (ranked by blast radius)

| # | Layer | File / Symbol | Metric | Headline |
|---:|---|---|---:|---|
| 1 | Rust | `crates/maestro-core/src/config.rs` | 2,693 LOC / 26 structs+enums / `Config` 12 fields | Workspace-wide configuration leviathan |
| 2 | Rust | `crates/maestro-web/src/routes/workflows.rs` | 2,313 LOC / 26 async handlers / 333-line `run_command_port_tracker` | One file owns every workflow endpoint |
| 3 | Rust | `crates/maestro-core/src/workflow/engine/step_runner.rs:268` | 429-line single function `run_agent_step_sequence` | Driver loop hides four state machines |
| 4 | Rust | `crates/maestro-core/src/container/runner.rs` | 1,513 LOC | Mounts, env, args, exec, cleanup in one struct |
| 5 | Rust | `crates/maestro-core/src/workflow/engine/types.rs` `Workflow` | 26 public fields | God data class shared across 6 sub-modules |
| 6 | Rust | `crates/maestro-core/src/github/auth_resolver.rs` | 1,381 LOC | Resolver, validator, tests, three mocks in one file |
| 7 | Rust | `crates/maestro-core/src/workflow/engine/mod.rs` | 1,319 LOC / `WorkflowEngine` 13 fields / 30+ public methods | Engine facade became a god |
| 8 | Rust | `crates/maestro-core/src/error.rs:13` | 16-variant enum, 11 wrap `String` | `thiserror` used but stringly inside |
| 9 | Rust | `crates/maestro-core/src/docker_hooks.rs` | 1,216 LOC / 19 `eprintln!` in prod | Hooks mixed with preflight + YAML parsing |
| 10 | Docker | `Dockerfile:3,14,40` | 3 `FROM`, 0 `@sha256` | Reproducibility breaks across base re-tags |
| 11 | React | `ui/src/components/modals/TicketDetailModal.tsx` | 501 LOC / 12 `useState` / 5 effects+refs | Modal does 6 jobs |
| 12 | React | `ui/src/pages/Dashboard.tsx` | 440 LOC / 21 hook calls / 10 `useState` | Page also orchestration container |

## Systemic smells

### [Rust] Stringly-typed `MaestroError`
- **Count:** 11/16 variants wrap `String`; 33 functions return `Result<_, String>`.
- **Root cause:** errors converted to text at the failure site (`.map_err(|e| MaestroError::Jira(e.to_string()))`).
- **Fix:** typed payloads on every domain variant; `#[from]` impls for `rusqlite::Error`, `reqwest::Error`, `serde_json::Error`; delete `Result<_, String>`.

### [Rust] Fire-and-forget `tokio::spawn`
- **Count:** 21 of 34 spawns drop the `JoinHandle`.
- **Root cause:** background tasks (Jira unassign, container cleanup, port tracking) spawned without graceful-shutdown plumbing.
- **Fix:** route all spawns through a `JoinSet` on `WorkflowEngine` / `AppState`; cancel via `CancellationToken` child.

### [Rust] Promiscuous `.clone()`
- **Count:** 793 in production; 140 in `routes/workflows.rs` alone.
- **Root cause:** moves through `tokio::spawn` and axum handler closures pay by cloning every captured field.
- **Fix:** group per-request shared values into a single `Arc<HandlerContext>` cloned once.

### [Rust] Lock-across-`.await`
- **Count:** 328 occurrences of `lock().await` / `read().await` / `write().await` — needs targeted audit.
- **Fix:** enable `clippy::await_holding_lock` workspace-wide; fix every flag.

### [Rust] God modules vs the `engine/` rule
- **Count:** 51 files over 400 LOC; CODING_STANDARDS §1.1 mandates split at ~300.
- **Fix:** apply `engine/`-style facade pattern to the 13 files over 800 LOC first; CI guard against new files > 600 LOC.

### [Rust] `eprintln!` in production paths
- **Count:** 42 occurrences in `src/`; `docker_hooks.rs` alone has 19.
- **Fix:** replace with `tracing::info!` / `tracing::warn!`.

### [React] State-rich page components
- **Count:** 4 files at 400+ LOC with 10–21 hooks each.
- **Fix:** extract domain-specific custom hooks; modal visibility belongs in a `useDashboardModals()` discriminated union.

### [React] `useEffect` for data loading
- **Count:** every page-level fetch in `Dashboard.tsx` (lines 119, 137, 145, 198, 221, 228) is a `useEffect`-on-mount pattern.
- **Fix:** introduce a small `useApi<T>(url, deps)` cache hook with in-flight dedupe.

### [Docker] Unpinned base images
- **Count:** 3 `FROM` lines, 0 `@sha256:` digests.
- **Fix:** pin all bases by digest; weekly Renovate bump.

### [Docker] Runtime bundles build toolchains
- **Count:** Rust toolchain (lines 122–136), `build-essential` + `autoconf` + `bison` + `libssl-dev` + `libyaml-dev` (lines 107–120).
- **Fix:** split into `maestro:slim` (no build tools) and `maestro:full` (current shape).

### [Docker] `@latest` in image build
- **Count:** 2 npm globals at `@latest` + `curl … | bash` for Cursor and Rust.
- **Fix:** pin all versions; verify checksums on direct downloads.

### [Docker] 29 `RUN` layers
- **Count:** 29 (26 in runtime stage).
- **Fix:** combine related apt installs + global npm installs; target ≤15 in runtime stage.

## Prioritised fix order (4 items, blast-radius first)

1. **Split `config.rs` and `routes/workflows.rs`** — 5,006 LOC of conflict-prone shared code; mechanical extraction along clean per-section/per-handler seams.
2. **Restructure `MaestroError`** — typed payloads on all variants; delete `Result<_, String>`; remove `Box<dyn Error>` from `run_server`.
3. **Pin Docker bases + split runtime image** — `@sha256:` on all `FROM`; publish `maestro:slim` and `maestro:full`; pin all `@latest` and shell-pipe installs.
4. **Audit `tokio::spawn` + `clippy::await_holding_lock`** — every fire-and-forget spawn registers in a `JoinSet`; lint catches lock-across-await.

## Per-layer cut plans

### `config.rs` → 8 files
- `config/agent.rs` — agent provider selection + per-step config
- `config/general.rs` — top-level toggles (concurrency caps, log level, polling)
- `config/git.rs` — git remote + GitHub App credentials
- `config/jira.rs` — Jira site, polling, prompt-mode policy
- `config/web.rs` — HTTP/WebSocket + runtime patches
- `config/runtime.rs` — Docker/Network/Editor/Terminal/Provisioning/Dev
- `config/template.rs` — `interpolate_*` and `shell_escape_value`
- `config/mod.rs` — `Config` aggregate, `Config::load`, re-exports

### `routes/workflows.rs` → 8 files
- `routes/workflows/dto.rs` — `WorkflowSummary`, `TerminalLineDto`, `RunCommandStatus`
- `routes/workflows/list.rs` — `list_workflows`, `workflow_counts`, `get_workflow`, `get_workflow_report`
- `routes/workflows/lifecycle.rs` — pause/resume/stop/retry/resume_from_error/mark_done/delete
- `routes/workflows/manual.rs` — `start_manual_workflow`
- `routes/workflows/editor.rs` — open/close editor + terminal
- `routes/workflows/run_commands.rs` — list/start/stop run commands
- `routes/workflows/definitions.rs` — list/run/retry workflow defs
- `routes/workflows/port_tracking.rs` — `track_port_forwards`

### `step_runner.rs::run_agent_step_sequence` (429 lines) → 4 functions
- `dispatch_provider_session(provider, ctx, prompt) -> StepOutcome`
- `build_step_prompt(step, ticket_ctx, vars) -> String`
- `apply_step_result(workflow, step_idx, outcome) -> Option<TerminalState>`
- `run_agent_step_sequence(...)` becomes a ~120-line outer/inner/repeat loop

### `Workflow` (26 fields) → 4 components
- `WorkflowIdentity` — id, key, summary, description, type, ticket_url, ticketing fields
- `WorkflowProgress` — state, steps_log, current_step_label, workflow_def_runs
- `WorkflowRuntime` — runtime metadata (cancel_token, terminal_lines, branch/worktree paths, driver_started, worktree_bootstrapped, pr_url/merged, started_manually, timestamps)
- `WorkflowOwnership` — user_id, auth_pin, last/description session ids, repository_id, workspace_name
- `Workflow = (Identity, Progress, Runtime, Ownership)` composition

### Dockerfile → 2 stages + 2 image targets
- `runtime-base` — `debian:bookworm-slim@<digest>` + minimal runtime deps + Playwright libs + maestro user + entrypoints + binary
- `runtime-build-tools` — `FROM runtime-base` + Rust toolchain + build-essential + autoconf + libssl-dev + libyaml-dev
- `maestro:slim` = `runtime-base`; `maestro:full` = `runtime-build-tools`
- All `npm install -g …@latest` pinned; checksums on `ttyd` / `openvscode-server` / Node tarballs; replace `curl | bash` for Cursor with pinned tarball + sha256

### `TicketDetailModal.tsx` (501 LOC) → 4 components
- `<TicketDetailHeader>` (~40 LOC)
- `<TicketDetailView>` (~80 LOC) — markdown + tabs + side-by-side
- `<TicketEditor>` (~100 LOC) — textarea + debounced preview + save
- `<TicketImproveWithAI>` (~140 LOC) — improving + countdown + abort + diff
- `<StartWorkflowFooter>` (~80 LOC, conditional)
- `<TicketDetailModal>` shell — ~80 LOC

### `Dashboard.tsx` (440 LOC, 21 hooks) → 1 shell + 4 hooks + 1 modals component
- `useOnboardingStatus()` — `systemStatus`, focus listener, legacy fallback
- `useMyRepositories()` — `myRepos`, `activeRepoName`, localStorage
- `useWorkflowDefinitions()` — `workflowDefs`, debounce ref, WS listener
- `useDashboardModals()` — discriminated union for picker/paste/nojira/detail/report
- `<DashboardModals>` — renders active modal from the union
- `<Dashboard>` — target ~120 LOC layout

## Project-rule adherence

| Rule | Status | Detail |
|---|---|---|
| No `unwrap()`/`expect()` in non-test code | **Fail (minor)** | 5 real code-path violations + ~45 in test-support files inside `src/` |
| `thiserror` errors; no `Box<dyn Error>` in public API | **Fail** | `MaestroError` variants stringly; `run_server` returns `Box<dyn Error>`; 33 `Result<_, String>` |
| Rust file > ~300 LOC ⇒ split | **Fail (systemic)** | 51 files exceed 400 LOC |
| React component > ~150 LOC ⇒ split | **Fail (4 hot spots)** | 4 over 400 LOC |
| No `RwLock`/`Mutex` guard across `.await` | **Indeterminate** | 328 sites match heuristic — needs `clippy::await_holding_lock` |
| TS `strict: true`, no `any`, no `@ts-ignore` | **Pass** | 0 / 0 / 0 |
| All API shapes in `src/api/types.ts` | **Pass** | 545 LOC, no inline anon types found |
| No `console.log` in merged code | **Pass (1 borderline)** | 1 `console.warn` in `MarkdownPreview.tsx:87` (mermaid sink) |
| No `println!` in production paths | **Fail** | 42 `println!`/`eprintln!` (19 in `docker_hooks.rs`) |
| Zero hardcoded secrets | **Pass** | No token-bearing `ARG`/`ENV` in Dockerfile; compose passes secrets as runtime env |
