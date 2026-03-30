# Maestro — context for AI coding agents

**New agent session:** this file must be the **first** project file you read (before `README.md`, `ARCHITECTURE.md`, or the crate tree). Cursor and Claude project rules enforce that.

This file is the **canonical high-level reference** for what the repository is, how it is structured, and how the main runtime paths work. Human-oriented detail lives in `README.md`, `ARCHITECTURE.md`, and `docs/workflow.md`; prefer those for setup, troubleshooting, and deep diagrams. **Keep this file aligned with the code** when you change behavior or layout (see `.cursor/rules/` and `CLAUDE.md`).

---

## What this project does

**Maestro** is a Rust application that **polls Jira** for work in **To Do**, then for each ticket runs an **automated pipeline**: assign / transition status, **clone work via a git worktree**, run install/lint/tests, drive **Claude Code** in **headless** mode to implement and review changes, optionally loop on failures with fix sessions, and **open a GitHub pull request** via `gh`. A small **web dashboard** lists workflows, streams terminal output over **WebSocket**, and exposes REST endpoints for control and config.

**Dry mode** (`[general] dry_mode = true` or CLI `--dry-run`) skips real Jira/GitHub side effects while still running local work (worktrees, commands, Claude) through the `ExternalActions` trait (`RealActions` vs `DryRunActions`).

---

## Repository layout

| Path | Role |
|------|------|
| `crates/maestro-core` | Domain logic: config, workflow engine and state machine, Jira (`acli`), git worktrees, `gh` PR/commits, process/Claude sessions, dry/real actions |
| `crates/maestro-web` | Axum app: `/api/*`, WebSocket `/ws`, embedded static UI under `src/assets/` |
| `crates/maestro-cli` | Binary entrypoint: load config, init tracing, construct engine + poller + router, `tokio::select!` for poller, HTTP server, and graceful shutdown |
| `docker/` | Container entrypoint, egress scripts, diagnostics |
| `config.toml.example` | Documented default config shape (local `config.toml` is often gitignored) |
| `docs/workflow.md` | Mermaid diagrams for ticket lifecycle and controls |

Workspace manifest: root `Cargo.toml` (Rust **2024** edition). Internal crates depend as `maestro-core` → used by `maestro-web` and `maestro-cli`.

---

## How the binary starts

`crates/maestro-cli/src/main.rs`:

1. Parses CLI (`--config` / `MAESTRO_CONFIG`, default `config.toml`; `--dry-run`).
2. Loads `Config` from file or defaults.
3. Initializes **JSON** logging via `tracing_subscriber` and `EnvFilter` (including `general.log_level`).
4. Builds `Arc<RwLock<Config>>` and `Arc<dyn ExternalActions>` from dry mode.
5. Constructs `WorkflowEngine`, `JiraPoller` with a shared `CancellationToken`, `AppState`, and `build_router`.
6. Runs poller, Axum server with graceful shutdown on cancel, and signal handler (`SIGINT` / `SIGTERM` on Unix). Shutdown cancels the token, calls `stop_all_workflows`, then waits briefly for cleanup.

---

## Workflow model

### State machine

Defined in `crates/maestro-core/src/workflow/state.rs` as `WorkflowState`: `Pending` → `Assigning` → `RetrievingDetails` → `CreatingWorktree` → repeated **`AddressingTicket { pass }` / `Reviewing`** (see below) → `Linting` → `UnitTesting` → `E2ETesting` → `CreatingPR` → `Done`, plus `Error { .. }`, `Paused { .. }`, `Stopped`.

Terminal states: `Done`, `Stopped`, `Error`.

### Engine and storage

`WorkflowEngine` (`workflow/engine.rs`) holds:

- `Arc<RwLock<Config>>`
- `Arc<RwLock<HashMap<String, Workflow>>>` — **keys are Jira ticket keys** (e.g. `PROJ-123`), not the workflow UUID
- `Arc<dyn ExternalActions>`
- `broadcast::Sender<WorkflowEvent>` for real-time updates

Each new ticket gets `WorkflowEngine::start_workflow`, which inserts the workflow and **`tokio::spawn`s `drive_workflow`** so each ticket runs concurrently, subject to `max_concurrent_workflows`.

`Workflow` carries ticket metadata, `steps_log`, branch/worktree paths, `pr_url`, `CancellationToken`, and up to **100** recent `terminal_lines` for UI persistence.

### Main step sequence (simplified)

Implemented in `run_workflow_steps` inside `workflow/engine.rs`:

1. **Assign** ticket (assignee name `"maestro"` in code) and move to **In Progress** (failures may be logged/`[DRY/SKIP]` but workflow can continue).
2. **Jira details** via `JiraClient` / `get_ticket_details`; populate description, summary, type on `Workflow`.
3. **Worktree** from `git::worktree::branch_name_for_ticket` and configured `base_branch`.
4. Optional **`pre_install`** then optional **`install`** shell commands in the worktree (streaming output).
5. **`address_ticket_passes`** iterations (from `[claude] address_ticket_passes`, default `3`): for each pass, **`ClaudeSession::start_address_ticket`** then **`PmAgent::validate_plan`** on the session output (separate `claude` process), then **`ClaudeSession::start_review_changes`**. Claude session IDs are **resumed** across passes via `--resume`.
6. **Lint / unit / e2e** phases run configured commands; on failure, **`ClaudeSession::start_fix_session`** may run up to `general.max_fix_attempts`, with commits between stages as implemented in the engine.
7. **Create PR** via `create_pr` on the actions trait (title/body/branch/base per implementation).

Pause/resume: workflow state wraps prior state in `Paused`; `wait_if_paused` blocks the driver until resumed.

Stop/shutdown: cancel token kills child processes (`ProcessHandle` uses process groups on Unix); Jira cleanup (unassign / To Do) is triggered from stop paths as implemented.

File logging: `WorkflowLogWriter` writes under `{repo_path}/logs/<TICKET>.log`.

---

## Claude Code integration

### Session runner

`crates/maestro-core/src/claude/session.rs` spawns the **`claude`** CLI with:

- `--dangerously-skip-permissions`, `--print`, `--verbose`, `-p <prompt>`, `--output-format stream-json`
- Optional `--resume <session_id>` for continuity
- Optional `--model <name>` when `[claude] model` is non-empty

Stdout is parsed: **session id** from the stream-json `system`/`init` line; human-readable **result** via `parse_stream_json_output`. Failures include empty stdout and non-zero exit.

### Prompts

- **Address ticket**: `/address-ticket` + ticket context plus **headless** instructions (no `AskUserQuestion`, autonomous approvals).
- **Review**: `/review-changes` plus headless instructions (address all findings, no interactive selection).
- **Fix**: free-form prompt with failing command output and instructions.

### PM agent

`crates/maestro-core/src/claude/pm_agent.rs` runs a **separate** short `claude --allow-dangerously-skip-permissions --print -p ...` invocation to return **APPROVED** or **REJECTED** against description + extracted acceptance criteria. The workflow logs the verdict; rejection does not automatically abort the pipeline unless combined with other failure logic (see engine).

---

## External actions boundary

`crates/maestro-core/src/actions/traits.rs` — `ExternalActions`:

- Jira: assign, transition, unassign, get ticket details (string payload / parsing in client)
- Git: create/remove worktree, create PR, **commit_changes**
- Shell: **run_command**

`real.rs` and `dry_run.rs` implement this for production vs dry runs.

---

## Web API and UI

`crates/maestro-web/src/server.rs` mounts:

| Method | Path | Notes |
|--------|------|--------|
| GET | `/api/workflows` | List summaries (includes `id` = workflow UUID and `ticket_key`) |
| GET | `/api/workflows/{id}` | **Path segment is the map key: Jira ticket key**, not the UUID `id` field |
| POST | `/api/workflows/{id}/pause` | Same: ticket key |
| POST | `/api/workflows/{id}/resume` | |
| POST | `/api/workflows/{id}/stop` | |
| POST | `/api/workflows/{id}/retry` | |
| GET/PUT | `/api/config` | Read/update TOML-backed config |
| GET | `/api/health` | |
| GET | `/ws` | WebSocket; JSON messages = `WorkflowEvent` (+ step/output fields) |

Static files: embedded from `crates/maestro-web/src/assets/` (e.g. `index.html`, `config.html`).

---

## Configuration (`Config`)

Loaded in `crates/maestro-core/src/config.rs` — sections:

- **`general`**: `dry_mode`, `poll_interval_secs`, `max_concurrent_workflows`, `max_fix_attempts`, `log_level`
- **`jira`**: `project_keys`, `item_types`, `jql_filter`, `site`, `email`
- **`git`**: `base_branch`, `repo_url`, `repo_path`
- **`commands`**: `pre_install`, `install`, `lint`, `unit_test`, `e2e_test`
- **`web`**: `host`, `port`
- **`claude`**: `skills_path`, `address_ticket_passes`, `step_timeout_secs`, `figma_api_token`, `model`
- **`network`**: `extra_egress_hosts`, **`allow_all_https`**

Runtime path defaults are described in `README.md` / `config.toml.example`.

---

## Jira poller

`crates/maestro-core/src/jira/poller.rs`: on an interval, if `project_keys` non-empty, lists **To Do** tickets (via `JiraClient` / `acli`), skips keys that already exist in the workflow map, respects `max_concurrent_workflows`, and calls `start_workflow`. Uses `cancel_token` for shutdown.

---

## Process management

`crates/maestro-core/src/process.rs`: `ProcessHandle::spawn`, streaming readers, timeouts, cancellation; Unix uses **process groups** so child trees can be killed together.

---

## Testing and quality

From repo root: `cargo build`, `cargo test`, `cargo check`.

---

## Maintaining this document

Whenever you change **crate boundaries**, **workflow sequencing**, **Claude flags/prompts**, **REST or WebSocket contracts**, **config fields**, or **Jira/git/PR behavior**, **update this file in the same task** if any section above becomes wrong or incomplete. Small typo-only edits elsewhere do not require updates.
