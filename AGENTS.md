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
| `workflows/` | Example standalone TOML files for ticket / PR-review / merge-base **`[[agent_steps]]`**-style lists (optional `[general] *_workflow_steps_file` paths) |
| `docs/workflow.md` | Mermaid diagrams for ticket lifecycle and controls |

Workspace manifest: root `Cargo.toml` (Rust **2024** edition). Internal crates depend as `maestro-core` → used by `maestro-web` and `maestro-cli`.

---

## How the binary starts

`crates/maestro-cli/src/main.rs`:

1. Parses CLI (`--config` / `MAESTRO_CONFIG`, default `config.toml`; `--dry-run`).
2. Loads `Config` from file or defaults.
3. Initializes **JSON** logging via `tracing_subscriber` and `EnvFilter` (including `general.log_level`).
4. Builds `Arc<RwLock<Config>>` and `Arc<dyn ExternalActions>` from dry mode.
5. Constructs `WorkflowEngine` with **`[general] max_concurrent_workflows`** (drives an internal semaphore for concurrent **mise** / **install** / **agent** sessions). Calls **`restore_persisted_workflows`** so workflows saved on the last graceful shutdown are loaded from **`{git.repo_path}/.maestro/workflow_snapshot.json`** before the poller’s first poll. The Jira poller separately enforces **`[general] max_active_workflows`** (see **Jira poller**).
6. Constructs `JiraPoller` with a shared `CancellationToken` and **`Arc<AtomicBool>`** polling pause flag (shared with `AppState`), then `build_router`.
7. Runs poller, Axum server with graceful shutdown on cancel, and signal handler (`SIGINT` / `SIGTERM` on Unix). On **graceful** shutdown the process cancels the token, **`persist_interrupt_for_restart`** (writes the snapshot for non-terminal workflows and cancels their driver tokens **without** Jira unassign / **`Stopped`**), then waits briefly for cleanup. **`POST …/stop`** and **`stop_all_workflows`** still unassign and move tickets back to **To Do** (explicit stop, not container resume).

---

## Workflow model

### State machine

Defined in `crates/maestro-core/src/workflow/state.rs` as `WorkflowState`: `Pending` → `Assigning` → `RetrievingDetails` → `CreatingWorktree` → repeated **`AddressingTicket { pass }`** (`pass` is the **outer cycle** index when the built-in step list is repeated via **`[claude] address_ticket_passes`**) → **`Done`**, plus **`AddressingPrComments { pass }`** for the optional **PR review** agent loop after **Done**, `Error { .. }`, `Paused { .. }`, `Stopped`. The **`Reviewing`** and **`CreatingPR`** variants remain for **deserialization of older persisted state**. **`WorkflowState::display_name()`** for **`AddressingTicket`** is the generic **`Running agent steps`**; for **`AddressingPrComments`** it is **`Addressing PR comments`**. The **dashboard and REST list** use **`Workflow::status_display()`**, which shows the live **`current_step_label`** (e.g. **`[PR review] Address PR feedback (run 1/1)`** or **`Implement ticket (cycle 2/3, run 1/1)`**).

Terminal states: `Done`, `Stopped`, `Error`.

### Engine and storage

`WorkflowEngine` (`workflow/engine.rs`) holds:

- `Arc<RwLock<Config>>`
- `Arc<RwLock<HashMap<String, Workflow>>>` — **keys are Jira ticket keys** (e.g. `PROJ-123`), not the workflow UUID
- `Arc<dyn ExternalActions>`
- `broadcast::Sender<WorkflowEvent>` for real-time updates

Each new ticket gets `WorkflowEngine::start_workflow`, which inserts the workflow and **`tokio::spawn`s `drive_workflow`**. **`max_concurrent_workflows`** caps concurrent **mise** / **install** / **agent** sessions via a shared semaphore (permits are taken **after** `wait_if_paused`, so paused workflows do not hold a heavy-work slot). The Jira poller uses **`max_active_workflows`** (or **`max_concurrent_workflows`** when **`max_active_workflows`** is **`0`**) against **`non_done_workflow_count`**: workflows whose state is **not** **`Done`** count as active (including **Paused**, **Stopped**, **Error**, and in-progress) until **Mark as Done** or **Delete** removes the row — **`Done`** rows do not consume an active slot for new To Do picks.

`Workflow` carries ticket metadata, `steps_log`, branch/worktree paths, `pr_url`, `CancellationToken`, and up to **100** recent `terminal_lines` for UI persistence. **`workflow/snapshot.rs`** defines the JSON snapshot format for graceful shutdown / restart (same **`.maestro/`** tree as **`outcome.toml`**). For a **manual** snapshot to bring back a **Done** workflow (Address PR / Merge base / Mark as Done), set **`state`** to **`Done`** in JSON (serde also accepts lowercase **`done`**), include a non-empty **`pr_url`**, and an existing **`worktree_path`** (absolute path inside the container). On restore, **`AddressingPrComments`** and **`MergingBaseBranch`** rows resume their respective secondary drivers; **`Done`** rows are inserted with **no** main driver.

### Main step sequence (simplified)

Implemented in `run_workflow_steps` inside `workflow/engine.rs`:

1. **Assign** ticket to the **currently authenticated Jira user** (`acli` **`@me`**, same as `JiraClient::assign_ticket`) and move to **In Progress** (failures may be logged/`[DRY/SKIP]` but workflow can continue). On **resume after restart**, if the workflow already has an on-disk **`worktree_path`**, the engine **skips** re-logging assign/retrieve/create and runs a light Jira sync (`sync_jira_for_resume`) instead, then continues from **`configure_git_author_from_github`**.
2. **Jira details** via `JiraClient` / `get_ticket_details`; populate description, summary, type on `Workflow` (full step logs on first run; refresh only on resume path above).
3. **Worktree** from `git::worktree::branch_name_for_ticket` and configured `base_branch` (skipped when resuming with an existing directory).
4. Optional **`pre_install`** (array of shell commands, or one string for backward compatibility) then optional **`install`** in the worktree (streaming output).
5. **Agent workflow** — Step lists can live in **`config.toml`** (root **`[[agent_steps]]`** before any **`[table]`**) or in a standalone file set by **`[general] ticket_workflow_steps_file`** (path relative to the main config file’s directory); the file contains only **`[[agent_steps]]`** entries. If **`[[agent_steps]]`** is empty after load: use **`config::default_agent_steps`** and repeat the full sequence **`[claude] address_ticket_passes`** times (default `3`). If non-empty: use **only** those steps; **`[claude] address_ticket_passes`** does **not** multiply the custom list — use each step’s **`repeat`** (≥ 1, default 1) to run that step multiple times in sequence (**`--resume`** after the first run). Interpolate **`{ticket_key}`**, **`{ticket_summary}`**, **`{ticket_description}`**, **`{ticket_type}`**, **`{acceptance_criteria}`**, **`{ticket_context}`**; append headless instructions from **`agent_prompt.rs`**. The **first** run of the workflow starts a **new** session; all later runs use **`--resume`**. There is **no** built-in PM / plan-validation pass — add a **custom step** whose **prompt** asks the agent to validate plans or requirements if you want that. Session failure **except** on the **last run of an outer cycle** **aborts**; failure on that last run is **non-fatal** (same as legacy review). There is **no** separate lint/unit/e2e command phase — if you want the linter or tests, add agent steps whose **prompts** instruct the tool to run and fix them.
6. **Finalize** — Read an optional PR URL via **`workflow::outcome::resolve_pr_url`**: prefer **`.maestro/outcome.toml`** in the worktree (`pr_url = "…"`), else the last agent session output line **`MAESTRO_PR_URL: …`**. If **no** PR URL is found **and** any **`steps_log`** entry is **`Failed`**, return **`Err`** (workflow ends in **`Error`**). If a PR URL **is** found, **`Failed`** steps in the log (e.g. non-fatal failure on the last agent run of a cycle) do **not** fail the workflow — a warning is logged and the run completes **`Done`**. Set **`workflow.pr_url`** when found; run **`gh pr edit --add-reviewer <login>`** for the authenticated **`gh`** user (best-effort — GitHub rejects if that user is already the PR author); append a **Workflow complete** step to **`steps_log`**; transition **`Done`**.

After the worktree is created, the engine calls **`configure_git_author_from_github`**: **`git config user.name` / `user.email`** in the worktree from **`gh api user`**, using GitHub’s **`{id}+{login}@users.noreply.github.com`** form so commits match the **`gh`** account.

### PR review workflow (after **Done**)

Triggered from the dashboard (**`POST /api/workflows/{ticket_key}/address-pr-comments`**) when the workflow is **`Done`**, **`pr_url`** is set, and the worktree path still exists. **`WorkflowEngine::start_pr_review_workflow`** sets **`AddressingPrComments`**, assigns a **fresh `CancellationToken`** (so a previously cancelled main-driver token cannot make the PR-review driver exit immediately at **`check_cancelled`**), then **`drive_pr_review_workflow`** runs **`[[review_agent_steps]]`** from config (optionally loaded from **`[general] review_workflow_steps_file`**) or built-in **`default_review_agent_steps`**, repeated **`[claude] review_address_ticket_passes`** times when that list is empty, via the same headless agent integration as the main loop. Prompts support **`{pr_url}`** plus the same placeholders as **`[[agent_steps]]`**. On success the workflow returns **`Done`**; on driver failure, **`Error`**. Step names are prefixed with **`[PR review]`** in **`steps_log`**.

**Merge base branch** (**`POST /api/workflows/{ticket_key}/merge-base-branch`**) is analogous: **`start_merge_base_workflow`** sets **`MergingBaseBranch`** and assigns a **fresh `CancellationToken`** before **`drive_merge_base_workflow`**. Steps come from **`[[merge_base_agent_steps]]`** or **`[general] merge_base_workflow_steps_file`**, else built-in defaults. Each dashboard run executes the full configured step list; prior **`[PR review]`** / **`[Merge base]`** successes do **not** skip agent sessions (unlike the **main** ticket flow, which still skips agent steps already **`Success`** in **`steps_log`** after container restart for resume).

### Mark as Done

**`POST /api/workflows/{ticket_key}/mark-done`** (dashboard **Mark as Done**): transitions Jira to **`[jira] done_status`** (default **`Done`**), then **`remove_worktree`**. Partial failure is returned in JSON (**`MarkDoneOutcome`**); the workflow is **removed from the map** only if **both** succeed. A WebSocket **`workflow_removed`** event is sent when the row is dropped.

**`POST /api/workflows/{ticket_key}/delete`** (dashboard **Delete**): allowed when **`WorkflowState::is_active()`** is false (not **running** — includes **Paused**, **Done**, **Stopped**, **Error**). Removes the row from the map, best-effort **`remove_worktree`**, worker container cleanup — **no Jira transitions**. Syncs **`workflow_snapshot.json`** best-effort; emits **`workflow_removed`**.

Pause/resume/stop apply while **`AddressingPrComments`** or **`MergingBaseBranch`** is active (**`is_active()`**).

Pause/resume: workflow state wraps prior state in `Paused`; `wait_if_paused` blocks the driver until resumed.

Stop (**API** / **`stop_all_workflows`**): cancel token kills child processes (`ProcessHandle` uses process groups on Unix); Jira cleanup (unassign / To Do) runs as today. **Graceful process shutdown** (SIGINT/SIGTERM path in **`maestro-cli`**) uses snapshot persist + cancel so tickets stay **In Progress** for resume.

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

The **index** dashboard (`crates/maestro-web/src/assets/app.js`, **`styles.css`**) renders each workflow card at a **fixed height** (600px in CSS so layout does not depend on Tailwind JIT seeing dynamically injected class names). The grid keeps a **stable ticket order** across refetches and full re-renders (pause/stop/complete): keys are **not** re-sorted by state; **new** workflows **append** at the end when they first appear in **`GET /api/workflows`**.

### Prompts

Templates live in **`[[agent_steps]]`** (`name`, **`prompt`**, **`repeat`**) and in **`[[review_agent_steps]]`** for the post-**Done** PR loop (same fields). Default steps use **generic natural-language** prompts (no slash-commands); teams with Claude skills can still put **`/address-ticket`** or **`/review-changes`** in a template. Placeholders: **`ticket_key`**, **`ticket_summary`**, **`ticket_description`**, **`ticket_type`**, **`acceptance_criteria`**, **`ticket_context`**, and **`pr_url`** (review workflow). Unknown **`{placeholders}`** are left unchanged.

---

## External actions boundary

`crates/maestro-core/src/actions/traits.rs` — `ExternalActions`:

- Jira: assign to current user (`@me`), transition, unassign, get ticket details (string payload / parsing in client)
- Git: create/remove worktree, **`create_pr`** (implemented on **`RealActions`/`DryRunActions`** but **not** called by the workflow engine — agents open PRs with **`gh`** or similar), **commit_changes**, **`configure_git_author_from_github`**, **`request_github_self_as_pr_reviewer`**
- Shell: **run_command**

Helpers: **`actions/gh_github.rs`** (`gh api user`, **`gh pr edit --add-reviewer`**). In **dry mode**, **`request_github_self_as_pr_reviewer`** does not call GitHub (returns skipped); **`configure_git_author_from_github`** still sets local **`git config`** when **`gh`** is available.

`real.rs` and `dry_run.rs` implement this for production vs dry runs.

---

## Web API and UI

`crates/maestro-web/src/server.rs` mounts:

| Method | Path | Notes |
|--------|------|--------|
| GET | `/api/workflows` | List summaries (includes `id` = workflow UUID, `ticket_key`, action flags such as **`can_delete`**, etc.); sorted by **`started_at`** ascending (oldest first — matches **first-load** dashboard order; the UI then **preserves** that order on updates and only **appends** new keys) |
| GET | `/api/workflows/{id}` | **Path segment is the map key: Jira ticket key**, not the UUID `id` field |
| POST | `/api/workflows/{id}/pause` | Same: ticket key |
| POST | `/api/workflows/{id}/resume` | |
| POST | `/api/workflows/{id}/stop` | |
| POST | `/api/workflows/{id}/retry` | |
| POST | `/api/workflows/{id}/address-pr-comments` | Start PR review agent loop (**`Done`** + **`pr_url`** + worktree) → **`202 Accepted`** |
| POST | `/api/workflows/{id}/merge-base-branch` | Start merge-base-branch agent loop (**`Done`** + **`pr_url`** + worktree) → **`202 Accepted`** |
| POST | `/api/workflows/{id}/mark-done` | Jira **`done_status`** + remove worktree; JSON **`MarkDoneOutcome`** |
| POST | `/api/workflows/{id}/delete` | Remove workflow when not **running**; no Jira change; **`workflow_removed`** on success |
| GET/PUT | `/api/config` | Read/update TOML-backed config |
| GET | `/api/polling` | JSON `{ "paused": bool }` — Jira poller pause state |
| POST | `/api/polling/pause` | Pause Jira polling (no new tickets picked up) |
| POST | `/api/polling/resume` | Resume Jira polling |
| GET | `/api/health` | |
| GET | `/ws` | WebSocket; JSON messages = `WorkflowEvent` (+ step/output fields); **`event_type`** may be **`workflow_removed`** when **Mark as Done** or **Delete** drops a workflow |

Static files: embedded from `crates/maestro-web/src/assets/` (e.g. `index.html`, `config.html`).

---

## Configuration (`Config`)

Loaded in `crates/maestro-core/src/config.rs` — sections:

- **Root (before any `[table]` in TOML)**: optional **`[[agent_steps]]`**, **`[[review_agent_steps]]`**, **`[[merge_base_agent_steps]]`** — same semantics as below. If **`[general] ticket_workflow_steps_file`** / **`review_workflow_steps_file`** / **`merge_base_workflow_steps_file`** is non-empty, that list is loaded from the given path (relative to **`config.toml`**’s directory) and the corresponding inline root tables in **`config.toml`** are **ignored** for that workflow type. Empty step list after load → built-in defaults for that type. See **`workflows/*.toml`** for file shape.
- **`general`**: `dry_mode`, `poll_interval_secs`, **`max_concurrent_workflows`** (semaphore for concurrent install/agent work), **`max_active_workflows`** (**`0`** = use **`max_concurrent_workflows`**; Jira poller counts workflows whose state is **not** **`Done`**), `log_level`, `worker_image`, optional **`ticket_workflow_steps_file`**, **`review_workflow_steps_file`**, **`merge_base_workflow_steps_file`**
- **`jira`**: `project_keys`, `item_types`, `jql_filter`, `site` (auth, egress; ticket context for prompts), `email`, **`done_status`** (Jira transition for **Mark as Done**)
- **`git`**: `base_branch`, `remote` (fetch / worktree / push; default `origin`), `repo_url`, `repo_path`
- **`commands`**: `pre_install` (`Vec<String>`, deserializes from a single string too), `install`
- **`web`**: `host`, `port`
- **`claude`**: `skills_path`, `address_ticket_passes` (how many times to run the **built-in** main step sequence when **`[[agent_steps]]`** is empty), **`review_address_ticket_passes`** (same for empty **`[[review_agent_steps]]`**), `step_timeout_secs`, `figma_api_token`, `model`
- **`agent`**: `provider` (`claude` \| `cursor`), `cursor_cli`, `cursor_model` (default `Auto`; Cursor CLI gets `--model Auto` unless a concrete id is set)
- **`docker`**: `build_commands` (image build), `compose_up_commands` (each `docker compose up`)
- **`network`**: `extra_egress_hosts`, **`allow_all_https`**

Runtime path defaults are described in `README.md` / `config.toml.example`. **`PUT /api/config`** replaces the in-memory **`Config`** from JSON and does **not** re-read **`*_workflow_steps_file`** paths from disk (use inline **`agent_steps`** / **`review_agent_steps`** / **`merge_base_agent_steps`** in the payload, or restart after editing files on disk).

---

## Jira poller

`crates/maestro-core/src/jira/poller.rs`: on an interval, if `project_keys` non-empty, lists **To Do** tickets (via `JiraClient` / `acli`), skips keys that already exist in the workflow map, and only starts new workflows when **`non_done_workflow_count`** is less than **`effective_max_active_workflows()`** (**`[general] max_active_workflows`**, or **`max_concurrent_workflows`** when **`max_active_workflows`** is **`0`**). **Done** workflows do not count toward this cap (so completed rows can remain on the dashboard without blocking new tickets). **Paused**, **Stopped**, **Error**, and in-progress rows still count until **Delete** or transition to **Done** + removal. **Restored** workflows are in the map before the first poll, so they consume active slots. Uses `cancel_token` for shutdown. When the dashboard sets **polling paused** (`AppState.polling_paused`), the poller still waits on the interval but **skips** `poll_once` (including the initial poll on startup if already paused). Polling pause is not persisted across process restarts; workflow snapshot resume is (see **How the binary starts**).

---

## Process management

`crates/maestro-core/src/process.rs`: `ProcessHandle::spawn`, streaming readers, timeouts, cancellation; Unix uses **process groups** so child trees can be killed together.

---

## Docker entrypoint and CLI helpers

- **`docker/entrypoint.sh`**: `setup` mode (required: `gh` + `acli`; optional: Claude, Cursor `agent login`, repo clone). Normal mode: **`maestro preflight`**, **`maestro docker-hooks startup`** (`[docker] compose_up_commands`), then **`exec maestro`** with image `CMD` args. Podman Compose often needs **`--podman-run-args="-i -t"`** for interactive setup (see README).
- **Container engine access**: the default Compose stack does **not** provide a Docker/Podman daemon. Image installs Debian’s **`docker.io`** package for the **`docker`** CLI (no in-container daemon). To give the CLI a daemon, merge **`docker-compose.dind.yml`** — a **Docker-in-Docker sidecar** that adds a **`docker:27-dind`** container running a real Docker daemon. Maestro connects via **`DOCKER_HOST=tcp://dind:2375`** over the internal compose network. The sidecar runs **`--privileged`** (required by DinD) but is isolated to its own container; Maestro itself gains no extra privileges. The **`workspace`** named volume is shared so paths like **`/workspace/my-project/…`** resolve identically from both containers. A **`dind-storage`** volume persists Docker image layers across restarts. The entrypoint waits for the DinD daemon to become ready (retries `docker info` against **`DOCKER_HOST`**) before starting Maestro. Works on all platforms (macOS Podman, macOS Docker Desktop, Linux). **`podman-compose`** (Python) does not read `COMPOSE_FILE` from `.env` — use explicit `-f` flags: `podman compose -f docker-compose.yml -f docker-compose.dind.yml up -d`.
- **Workflow isolation**: when DinD is available, each workflow’s **install**, **pre_install**, **mise install**, and **agent steps** (Claude/Cursor sessions) run in **ephemeral Docker containers** (`docker run --rm`) instead of directly on the Maestro host. Each worker container gets its own network namespace (no port conflicts between concurrent workflows) and a `--cap-add=NET_ADMIN` for egress rules. The worker image is set via **`[general] worker_image`** or auto-detected from the running Maestro container (`docker inspect maestro`); default fallback is **`maestro:latest`**. After `make up`, load the image into DinD: **`make load-worker`** (`podman save | docker load`). Auth volumes are mounted into DinD at **`/shared-auth/*`** and bind-mounted into workers at the standard home paths. A **`playwright-cache`** volume maps to **`/shared-auth/playwright-cache`** (DinD) and **`~/.cache/ms-playwright`** in workers so Playwright uses the **project’s** browser revision from **`npm`/`playwright install`**, not a mismatched Chromium baked into the image (which caused visual snapshot drift). Workers default **`TZ=UTC`**, **`LANG`/`LC_ALL=C.UTF-8`**; the host may pass through **`CI`**, **`TZ`**, **`LANG`**, **`LC_ALL`**, **`PLAYWRIGHT_BROWSERS_PATH`** when set. The worker entrypoint (**`worker-entrypoint.sh`**) applies egress rules, sources **`/etc/maestro/env`**, then `runuser`s as **`maestro`**. Direct commands (like `claude`, `mise`) bypass the entrypoint (`--entrypoint ""`). Worker containers are force-removed on **cancel**, **stop**, **mark-done**, **delete**, and **stop_all_workflows** via **`ContainerRunner::cleanup_for_ticket`**. When DinD is unavailable (no `DOCKER_HOST` or `docker info` fails), the engine logs a warning and falls back to **local execution** (no isolation).
- **`maestro preflight`**: validates GitHub, Atlassian, and provider-specific auth. Cursor: skips **`agent status`** when **`CURSOR_API_KEY`** is set or when **`cli-config.json`** under **`CURSOR_CONFIG_DIR`** looks authenticated; otherwise **`agent status`** with timeout and process-group kill. Compose sets **`CURSOR_CONFIG_DIR=/home/maestro/.cursor`** to align with the **`cursor-auth`** volume.
- **`maestro docker-hooks build|startup`**: runs `build_commands` or `compose_up_commands` from config as **`bash -c`** in `git.repo_path` (used by Dockerfile `RUN` and entrypoint; **`sh`** on Debian is often dash and lacks `pipefail`). Hook children get **`HOME`**, **`MAESTRO_HOME`** (compose: `/home/maestro`), and **`CURSOR_CONFIG_DIR`** so writes to **`~/.claude`** / **`~/.cursor`** hit named volumes, not **`/.claude`** when **`HOME`** is unset.

## Testing and quality

From repo root: `cargo build`, `cargo test`, `cargo check`.

---

## Maintaining this document

Whenever you change **crate boundaries**, **workflow sequencing**, **Claude flags/prompts**, **REST or WebSocket contracts**, **config fields**, **Docker entrypoint/setup or `[docker]` hooks**, or **Jira/git/PR behavior**, **update this file in the same task** if any section above becomes wrong or incomplete. Small typo-only edits elsewhere do not require updates.
