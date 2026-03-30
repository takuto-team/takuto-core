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
| `crates/maestro-core` | Domain logic: config, workflow engine and state machine, Jira (`acli`), git worktrees, `gh` PR/commits, Claude/Cursor agent sessions, process management, dry/real actions |
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
5. **`address_ticket_passes`** iterations (from `[claude] address_ticket_passes`, default `3`): for each pass, the engine runs either **`ClaudeSession`** (Claude Code CLI) or **`CursorSession`** (`cursor/session.rs`, Cursor Agent CLI) based on **`[agent] provider`** (`claude` | `cursor`). Then **`PmAgent::validate_plan`** runs the same provider for a short plan check, then address/review resume the same session via **`--resume`** where supported.
6. **Lint / unit / e2e** phases run configured commands; on failure, the configured provider’s **fix session** runs up to `general.max_fix_attempts`, with commits between stages as implemented in the engine.
7. **Create PR** via `create_pr` on the actions trait (title/body/branch/base per implementation).

Pause/resume: workflow state wraps prior state in `Paused`; `wait_if_paused` blocks the driver until resumed.

Stop/shutdown: cancel token kills child processes (`ProcessHandle` uses process groups on Unix); Jira cleanup (unassign / To Do) is triggered from stop paths as implemented.

File logging: `WorkflowLogWriter` writes under `{repo_path}/logs/<TICKET>.log`.

---

## AI agent integration (Claude Code or Cursor Agent)

### Provider selection

`[agent] provider` in config: **`claude`** (default) or **`cursor`**. **`[agent] cursor_cli`** sets the Cursor Agent executable (default **`agent`**). **`[claude] model`** is passed to both CLIs when non-empty.

### Claude Code

`crates/maestro-core/src/claude/session.rs` spawns **`claude`** with `--dangerously-skip-permissions`, `--print`, `--verbose`, `-p`, `--output-format stream-json`, optional `--resume`, optional `--model`.

### Cursor Agent (headless)

`crates/maestro-core/src/cursor/session.rs` spawns **`cursor_cli`** (e.g. `agent`) with **print** mode, **`--output-format stream-json`**, **`--stream-partial-output`**, **`--trust`**, **`--force`**, **`--approve-mcps`**, **`--sandbox disabled`**, **`--workspace <worktree>`**, optional **`--resume`**, optional **`--model`**. This matches Cursor’s non-interactive / trusted automation flags from their CLI docs.

### Dashboard streaming

Raw stdout lines are turned into short lines for the web UI via **`workflow/stream_humanize.rs`**: **`humanize_agent_stream_line`** dispatches on provider. Cursor stream-json events include **assistant** text, **`tool_call` started** (e.g. read/write paths), and **result** — so operators still see **live progress** similar to Claude. (Cursor’s docs note **thinking** events are suppressed in print mode; internal chain-of-thought is not shown, but tool use and assistant text are.)

### Prompts

- **Claude address**: `/address-ticket` + ticket context + headless instructions.
- **Claude review**: `/review-changes` + headless instructions.
- **Cursor address/review**: Natural-language task prompts derived from the same ticket context (no slash-commands); headless instructions included in the prompt.

### PM agent

`crates/maestro-core/src/claude/pm_agent.rs` validates plans using **`claude`** or the **Cursor CLI** depending on **`[agent] provider`**. The workflow logs **APPROVED** / **REJECTED**; rejection does not automatically abort the pipeline (see engine).

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
- **`agent`**: `provider` (`claude` \| `cursor`), `cursor_cli`
- **`docker`**: `build_commands` (image build), `compose_up_commands` (each `docker compose up`)
- **`network`**: `extra_egress_hosts`, **`allow_all_https`**

Runtime path defaults are described in `README.md` / `config.toml.example`.

---

## Jira poller

`crates/maestro-core/src/jira/poller.rs`: on an interval, if `project_keys` non-empty, lists **To Do** tickets (via `JiraClient` / `acli`), skips keys that already exist in the workflow map, respects `max_concurrent_workflows`, and calls `start_workflow`. Uses `cancel_token` for shutdown.

---

## Process management

`crates/maestro-core/src/process.rs`: `ProcessHandle::spawn`, streaming readers, timeouts, cancellation; Unix uses **process groups** so child trees can be killed together.

---

## Docker entrypoint and CLI helpers

- **`docker/entrypoint.sh`**: `setup` mode (required: `gh` + `acli`; optional: Claude, Cursor `agent login`, repo clone). Normal mode: **`maestro preflight`**, **`maestro docker-hooks startup`** (`[docker] compose_up_commands`), then **`exec maestro`** with image `CMD` args.
- **`maestro preflight`**: validates GitHub, Atlassian, and provider-specific auth (`claude auth status` or `agent status`, unless `CURSOR_API_KEY` is set for Cursor).
- **`maestro docker-hooks build|startup`**: runs `build_commands` or `compose_up_commands` from config as `sh -c` in `git.repo_path` (used by Dockerfile `RUN` and entrypoint).

## Testing and quality

From repo root: `cargo build`, `cargo test`, `cargo check`.

---

## Maintaining this document

Whenever you change **crate boundaries**, **workflow sequencing**, **Claude flags/prompts**, **REST or WebSocket contracts**, **config fields**, **Docker entrypoint/setup or `[docker]` hooks**, or **Jira/git/PR behavior**, **update this file in the same task** if any section above becomes wrong or incomplete. Small typo-only edits elsewhere do not require updates.
