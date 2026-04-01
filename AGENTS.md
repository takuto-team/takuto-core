# Maestro — context for AI coding agents

**New agent session:** this file must be the **first** project file you read (before `README.md`, `ARCHITECTURE.md`, or the crate tree). Cursor and Claude project rules enforce that.

This file is the **canonical high-level reference** for what the repository is, how it is structured, and how the main runtime paths work. Human-oriented detail lives in `README.md`, `ARCHITECTURE.md`, and `docs/workflow.md`; prefer those for setup, troubleshooting, and deep diagrams. **Keep this file aligned with the code** when you change behavior or layout (see `.cursor/rules/` and `CLAUDE.md`).

---

## What this project does

**Maestro** is a Rust application that **polls Jira** for work in **To Do**, then for each ticket runs an **automated pipeline**: assign / transition status, **clone work via a git worktree**, optional **pre_install** / **install**, then configurable **`[[agent_steps]]`** sessions (Claude Code or Cursor Agent in **headless** mode — implement, review, lint/tests, **open a PR with `gh`**, or any custom sequence). The engine **does not** run a separate PR step: when the last agent session exits successfully, the workflow **finalizes** to **`Done`**. An optional PR URL for the dashboard comes from **`.maestro/outcome.toml`** (`pr_url = "…"`) or a stdout line **`MAESTRO_PR_URL: …`** (see **`agent_prompt`** headless suffix). A small **web dashboard** lists workflows, streams terminal output over **WebSocket**, and exposes REST endpoints for control and config.

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

Defined in `crates/maestro-core/src/workflow/state.rs` as `WorkflowState`: `Pending` → `Assigning` → `RetrievingDetails` → `CreatingWorktree` → repeated **`AddressingTicket { pass }`** (`pass` is the **outer cycle** index when the built-in step list is repeated via **`[claude] address_ticket_passes`**) → **`Done`**, plus `Error { .. }`, `Paused { .. }`, `Stopped`. The **`Reviewing`** and **`CreatingPR`** variants remain for **deserialization of older persisted state**. **`WorkflowState::display_name()`** for **`AddressingTicket`** is the generic **`Running agent steps`**; the **dashboard and REST list** use **`Workflow::status_display()`**, which shows the live **`current_step_label`** (e.g. **`Implement ticket (cycle 2/3, run 1/1)`** from configured **`[[agent_steps]]`** names, `repeat`, and outer loops).

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

1. **Assign** ticket to the **currently authenticated Jira user** (`acli` **`@me`**, same as `JiraClient::assign_ticket`) and move to **In Progress** (failures may be logged/`[DRY/SKIP]` but workflow can continue).
2. **Jira details** via `JiraClient` / `get_ticket_details`; populate description, summary, type on `Workflow`.
3. **Worktree** from `git::worktree::branch_name_for_ticket` and configured `base_branch`.
4. Optional **`pre_install`** (array of shell commands, or one string for backward compatibility) then optional **`install`** in the worktree (streaming output).
5. **Agent workflow** — If **`[[agent_steps]]`** is empty: use **`config::default_agent_steps`** and repeat the full sequence **`[claude] address_ticket_passes`** times (default `3`). If **`[[agent_steps]]`** is non-empty: use **only** those steps (built-in sequence is not used); **`[claude] address_ticket_passes`** does **not** multiply the custom list — use each step’s **`repeat`** (≥ 1, default 1) to run that step multiple times in sequence (**`--resume`** after the first run). Interpolate **`{ticket_key}`**, **`{ticket_summary}`**, **`{ticket_description}`**, **`{ticket_type}`**, **`{acceptance_criteria}`**, **`{ticket_context}`**; append headless instructions from **`agent_prompt.rs`**. The **first** run of the workflow starts a **new** session; all later runs use **`--resume`**. There is **no** built-in PM / plan-validation pass — add a **custom step** whose **prompt** asks the agent to validate plans or requirements if you want that. Session failure **except** on the **last run of an outer cycle** **aborts**; failure on that last run is **non-fatal** (same as legacy review). Put **`[[agent_steps]]`** at the **TOML root before any `[section]`** when you use it. There is **no** separate lint/unit/e2e command phase — if you want the linter or tests, add agent steps whose **prompts** instruct the tool to run and fix them.
6. **Finalize** — If any **`steps_log`** entry has **`Failed`**, return **`Err`** (workflow ends in **`Error`**). Otherwise read an optional PR URL via **`workflow::outcome::resolve_pr_url`**: prefer **`.maestro/outcome.toml`** in the worktree (`pr_url = "…"`), else the last agent session output line **`MAESTRO_PR_URL: …`**. Set **`workflow.pr_url`** when found; append a **Workflow complete** step to **`steps_log`**; transition **`Done`**.

Pause/resume: workflow state wraps prior state in `Paused`; `wait_if_paused` blocks the driver until resumed.

Stop/shutdown: cancel token kills child processes (`ProcessHandle` uses process groups on Unix); Jira cleanup (unassign / To Do) is triggered from stop paths as implemented.

File logging: `WorkflowLogWriter` writes under `{repo_path}/logs/<TICKET>.log`.

---

## AI agent integration (Claude Code or Cursor Agent)

### Provider selection

`[agent] provider` in config: **`claude`** (default) or **`cursor`**. **`[agent] cursor_cli`** sets the Cursor Agent executable (default **`agent`**). **`[claude] model`** is passed to Claude Code when non-empty. **`[agent] cursor_model`** (default **`Auto`**) sets Cursor Agent `--model`; **`Auto`** (any ASCII case) or empty means automatic model selection (passed as `--model Auto`).

### Claude Code

`crates/maestro-core/src/claude/session.rs` spawns **`claude`** with `--dangerously-skip-permissions`, `--print`, `--verbose`, `-p`, `--output-format stream-json`, optional `--resume`, optional `--model`.

### Cursor Agent (headless)

`crates/maestro-core/src/cursor/session.rs` spawns **`cursor_cli`** (e.g. `agent`) with **print** mode, **`--output-format stream-json`**, **`--stream-partial-output`**, **`--trust`**, **`--force`**, **`--approve-mcps`**, **`--sandbox disabled`**, **`--workspace <worktree>`**, optional **`--resume`**, optional **`--model`**. This matches Cursor’s non-interactive / trusted automation flags from their CLI docs.

The **Docker image** installs **Node.js 23+** from **nodejs.org** (not Node 20): the Cursor **`cursor-agent`** launcher runs its bundled **`node`** with **`--use-system-ca`**, which requires **Node ≥ 23.9** on Linux; older Node fails with `bad option: --use-system-ca`. The image copies the **full** Cursor Agent package under **`/usr/local/share/cursor-agent`** and symlinks **`/usr/local/bin/agent`** to that launcher (copying only the script to **`/usr/local/bin`** breaks **`index.js`** resolution).

The image also installs **mise** (apt). **`process::worktree_has_mise_config`** detects **`.mise.toml`**, **`mise.toml`**, **`.tool-versions`**, or **`.config/mise/config.toml`**; the workflow runs **`mise install`** in the worktree, then **`run_shell_command` / `run_shell_command_streaming`** use **`mise exec -- sh -c …`** when that detection matches. **`run_command`** (argv, e.g. **`acli`**) is unchanged.

### Dashboard streaming

Raw stdout lines are turned into short lines for the web UI via **`workflow/stream_humanize.rs`**: **`humanize_agent_stream_line`** dispatches on provider. Cursor stream-json events include **assistant** text, **`tool_call` started** (e.g. read/write paths), and **result** — so operators still see **live progress** similar to Claude. (Cursor’s docs note **thinking** events are suppressed in print mode; internal chain-of-thought is not shown, but tool use and assistant text are.)

### Prompts

Templates live in **`[[agent_steps]]`** (`name`, **`prompt`**, **`repeat`**). Default steps use **generic natural-language** prompts (no slash-commands); teams with Claude skills can still put **`/address-ticket`** or **`/review-changes`** in a template. Unknown **`{placeholders}`** are left unchanged.

---

## External actions boundary

`crates/maestro-core/src/actions/traits.rs` — `ExternalActions`:

- Jira: assign to current user (`@me`), transition, unassign, get ticket details (string payload / parsing in client)
- Git: create/remove worktree, **`create_pr`** (implemented on **`RealActions`/`DryRunActions`** but **not** called by the workflow engine — agents open PRs with **`gh`** or similar), **commit_changes**
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

- **Root (before any `[table]` in TOML)**: optional **`[[agent_steps]]`** (`name`, `prompt`, **`repeat`** default `1`) — replaces built-in steps when any are defined; empty → built-in two-step sequence repeated **`[claude] address_ticket_passes`** times
- **`general`**: `dry_mode`, `poll_interval_secs`, `max_concurrent_workflows`, `log_level`
- **`jira`**: `project_keys`, `item_types`, `jql_filter`, `site` (auth, egress; ticket context for prompts), `email`
- **`git`**: `base_branch`, `remote` (fetch / worktree / push; default `origin`), `repo_url`, `repo_path`
- **`commands`**: `pre_install` (`Vec<String>`, deserializes from a single string too), `install`
- **`web`**: `host`, `port`
- **`claude`**: `skills_path`, `address_ticket_passes` (how many times to run the **built-in** step sequence when **`[[agent_steps]]`** is empty), `step_timeout_secs`, `figma_api_token`, `model`
- **`agent`**: `provider` (`claude` \| `cursor`), `cursor_cli`, `cursor_model` (default `Auto`; Cursor CLI gets `--model Auto` unless a concrete id is set)
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

- **`docker/entrypoint.sh`**: `setup` mode (required: `gh` + `acli`; optional: Claude, Cursor `agent login`, repo clone). Normal mode: **`maestro preflight`**, **`maestro docker-hooks startup`** (`[docker] compose_up_commands`), then **`exec maestro`** with image `CMD` args. Podman Compose often needs **`--podman-run-args="-i -t"`** for interactive setup (see README).
- **`maestro preflight`**: validates GitHub, Atlassian, and provider-specific auth. Cursor: skips **`agent status`** when **`CURSOR_API_KEY`** is set or when **`cli-config.json`** under **`CURSOR_CONFIG_DIR`** looks authenticated; otherwise **`agent status`** with timeout and process-group kill. Compose sets **`CURSOR_CONFIG_DIR=/home/maestro/.cursor`** to align with the **`cursor-auth`** volume.
- **`maestro docker-hooks build|startup`**: runs `build_commands` or `compose_up_commands` from config as **`bash -c`** in `git.repo_path` (used by Dockerfile `RUN` and entrypoint; **`sh`** on Debian is often dash and lacks `pipefail`). Hook children get **`HOME`**, **`MAESTRO_HOME`** (compose: `/home/maestro`), and **`CURSOR_CONFIG_DIR`** so writes to **`~/.claude`** / **`~/.cursor`** hit named volumes, not **`/.claude`** when **`HOME`** is unset.

## Testing and quality

From repo root: `cargo build`, `cargo test`, `cargo check`.

---

## Maintaining this document

Whenever you change **crate boundaries**, **workflow sequencing**, **Claude flags/prompts**, **REST or WebSocket contracts**, **config fields**, **Docker entrypoint/setup or `[docker]` hooks**, or **Jira/git/PR behavior**, **update this file in the same task** if any section above becomes wrong or incomplete. Small typo-only edits elsewhere do not require updates.
