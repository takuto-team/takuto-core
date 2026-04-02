# Maestro Architecture

## Overview

Maestro is a Rust application that automates Jira ticket handling using **Claude Code** or **Cursor Agent** in headless mode. It polls Jira for tickets, orchestrates a workflow per ticket (branching, install hooks, configurable **`[[agent_steps]]`** sessions, PR creation), and serves a web dashboard for real-time monitoring and control. Lint and test gates are expressed as **agent prompts**, not separate `[commands]` fields.

---

## Project Structure

Cargo workspace with three crates:

```
maestro/
├── Cargo.toml                  # workspace root
├── Cargo.lock
├── ARCHITECTURE.md
├── config.toml                 # default configuration
├── Dockerfile
├── docker-compose.yml
├── crates/
│   ├── maestro-core/           # workflow engine, orchestrator, external integrations
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── config.rs       # TOML config deserialization
│   │       ├── workflow/
│   │       │   ├── mod.rs
│   │       │   ├── state.rs    # state machine definition
│   │       │   ├── engine.rs   # workflow orchestrator
│   │       │   └── step.rs     # individual step execution
│   │       ├── jira/
│   │       │   ├── mod.rs
│   │       │   ├── poller.rs   # polling loop
│   │       │   └── client.rs   # acli wrapper
│   │       ├── claude/
│   │       │   ├── mod.rs
│   │       │   └── session.rs  # headless session management
│   │       ├── git/
│   │       │   ├── mod.rs
│   │       │   ├── worktree.rs # worktree creation/cleanup
│   │       │   └── pr.rs       # PR creation via gh
│   │       ├── actions/
│   │       │   ├── mod.rs
│   │       │   ├── traits.rs   # ExternalActions trait
│   │       │   ├── real.rs     # RealActions implementation
│   │       │   └── dry_run.rs  # DryRunActions implementation
│   │       ├── process.rs      # child process management
│   │       └── error.rs        # error types
│   ├── maestro-web/            # axum web server + dashboard
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── server.rs       # axum app setup
│   │       ├── routes/
│   │       │   ├── mod.rs
│   │       │   ├── workflows.rs  # workflow CRUD + control
│   │       │   ├── config.rs     # config CRUD
│   │       │   └── ws.rs         # WebSocket handler
│   │       ├── state.rs        # shared app state
│   │       └── assets/         # static frontend files
│   │           ├── index.html
│   │           ├── app.js
│   │           └── styles.css
│   └── maestro-cli/            # binary entry point
│       ├── Cargo.toml
│       └── src/
│           └── main.rs         # CLI args, config loading, startup
└── tests/
    └── integration/            # integration tests
```

### Crate responsibilities

| Crate | Purpose |
|---|---|
| `maestro-core` | All business logic: workflow state machine, Jira polling, Claude Code session management, git operations, process management. No web concerns. |
| `maestro-web` | HTTP server, REST API, WebSocket push, static asset serving. Depends on `maestro-core`. |
| `maestro-cli` | Binary entry point. Parses CLI args, loads config, starts the core engine and web server. Depends on both other crates. |

---

## Key Dependencies

```toml
# maestro-core
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
toml = "0.8"
tracing = "0.1"
tracing-subscriber = "0.3"
thiserror = "2"
uuid = { version = "1", features = ["v4"] }
chrono = { version = "0.4", features = ["serde"] }

# maestro-web
axum = { version = "0.8", features = ["ws"] }
axum-extra = "0.10"
tower = "0.5"
tower-http = { version = "0.6", features = ["cors", "fs"] }
rust-embed = "8"
tokio-tungstenite = "0.26"

# maestro-cli
clap = { version = "4", features = ["derive"] }
```

---

## Workflow Orchestrator

### State Machine

Each ticket gets its own state machine instance. States:

```
Pending
  → Assigning
    → RetrievingDetails
      → CreatingWorktree
        → AddressingTicket { pass }   (agent steps; pass indexes outer cycles when using built-in sequence)
          → CreatingPR
            → Done

Any state → Error { source_state, message }
Any state → Paused { source_state }
Any state → Stopped
```

The `Reviewing` enum variant is kept for **deserializing older persisted workflows**; new runs use `AddressingTicket` for main agent steps and **`AddressingPrComments`** for the optional post-**Done** PR review loop.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkflowState {
    Pending,
    Assigning,
    RetrievingDetails,
    CreatingWorktree,
    AddressingTicket { pass: u8 },
    AddressingPrComments { pass: u8 },
    Reviewing,
    CreatingPR,
    Done,
    Error {
        source_state: Box<WorkflowState>,
        message: String,
    },
    Paused {
        source_state: Box<WorkflowState>,
    },
    Stopped,
}
```

### Workflow Engine

The engine is the main loop that drives all workflows:

```rust
pub struct WorkflowEngine {
    config: Arc<Config>,
    workflows: Arc<RwLock<HashMap<String, Workflow>>>,
    actions: Arc<dyn ExternalActions>,
    event_tx: broadcast::Sender<WorkflowEvent>,
}

pub struct Workflow {
    pub id: String,
    pub ticket_key: String,
    pub state: WorkflowState,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub steps_log: Vec<StepLog>,
    pub branch_name: String,
    pub worktree_path: Option<PathBuf>,
    pub cancel_token: CancellationToken,
}
```

The engine spawns one `tokio::task` per workflow. Each task drives its workflow through the state machine by executing the current step and transitioning on success/failure.

### Step Execution

Each state maps to a step executor:

```rust
#[async_trait]
pub trait StepExecutor: Send + Sync {
    async fn execute(&self, workflow: &mut Workflow, actions: &dyn ExternalActions) -> Result<WorkflowState>;
}
```

Steps log their output to `workflow.steps_log` for the report view.

---

## Process Management

Claude Code / Cursor Agent headless sessions and other shell hooks (`pre_install`, `install`, etc.) run as child processes managed through `tokio::process::Command`.

```rust
pub struct ProcessHandle {
    child: Child,
    stdout_lines: Vec<String>,
    stderr_lines: Vec<String>,
    cancel_token: CancellationToken,
}
```

Key design points:

- **Cancellation**: Each workflow holds a `tokio_util::sync::CancellationToken`. On stop/pause, the token is cancelled, which signals all child processes to terminate (SIGTERM, then SIGKILL after timeout).
- **Output capture**: stdout/stderr are captured line-by-line for logging and the execution report.
- **Timeout**: Each step has a configurable timeout. If exceeded, the process is killed and the workflow transitions to Error.

### Claude Code Session

```rust
pub struct ClaudeSession {
    process: ProcessHandle,
    session_id: String,
}

impl ClaudeSession {
    pub async fn start(worktree_path: &Path, skill: &str, config: &Config) -> Result<Self> {
        // claude --dangerously-skip-permissions
        //        --print
        //        -p "<skill invocation prompt>"
        //        --output-format stream-json
    }
}
```

The session uses `--print` mode with `--output-format stream-json` to capture structured output. The JSON stream is parsed line-by-line to extract progress, results, and errors.

Plan validation or “PM review” behavior is not a separate subprocess: teams add **`[[agent_steps]]`** entries whose **prompts** ask the agent to validate against **`{acceptance_criteria}`** or similar, in the same headless session model as other steps.

---

## Web Server

### Stack

- **Backend**: axum with tokio runtime
- **Frontend**: vanilla JS + Tailwind CSS (via CDN), embedded in binary via `rust-embed`
- **Real-time**: WebSocket for live workflow updates

### REST API

```
GET    /api/workflows              # list all workflows
GET    /api/workflows/:id          # get workflow details + step log
POST   /api/workflows/:id/pause    # pause workflow
POST   /api/workflows/:id/resume   # resume workflow
POST   /api/workflows/:id/stop     # stop workflow
POST   /api/workflows/:id/retry  # restart workflow from scratch
POST   /api/workflows/:id/address-pr-comments  # PR review agent loop (after Done + pr_url)
POST   /api/workflows/:id/mark-done  # Jira Done + remove worktree (JSON outcome)

GET    /api/config                 # get current config
PUT    /api/config                 # update config (partial)

GET    /api/health                 # health check

GET    /api/polling                # { "paused": bool }
POST   /api/polling/pause          # pause Jira poller
POST   /api/polling/resume         # resume Jira poller

WS     /ws                        # WebSocket for real-time events
```

### WebSocket Events

```json
{
  "type": "workflow_updated",
  "workflow_id": "PROJ-123",
  "state": "Implement ticket (cycle 1/3, run 1/1)",
  "timestamp": "2026-03-27T10:00:00Z"
}
```

```json
{
  "type": "workflow_error",
  "workflow_id": "PROJ-123",
  "error": "Agent step failed — check provider auth",
  "timestamp": "2026-03-27T10:01:00Z"
}
```

Events are broadcast via `tokio::sync::broadcast` from the workflow engine to all connected WebSocket clients.

### Shared Application State

```rust
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tokio::sync::RwLock;

pub struct AppState {
    pub engine: Arc<WorkflowEngine>,
    pub config: Arc<RwLock<Config>>,
    pub polling_paused: Arc<AtomicBool>,
}
```

Passed to all axum handlers via `axum::extract::State`.

---

## Frontend

Vanilla JS + Tailwind CSS, served as embedded static assets.

### Pages

1. **Dashboard** (`/`)
   - Grid of workflow cards
   - Header: WebSocket status, **Pause / Resume Jira polling**, link to config
   - Each card shows: ticket key (linked to Jira), current state with visual indicator, elapsed time, error message if any
   - Action buttons: Pause/Resume, Stop
   - Report button opens modal with full execution log
   - Auto-updates via WebSocket

2. **Configuration** (`/config`)
   - Form with all configurable fields
   - Save button sends `PUT /api/config`
   - Validates before submit

3. **Report Modal**
   - Step-by-step execution log
   - Expandable sections per step with stdout/stderr
   - Duration per step
   - Final status

### Asset Embedding

```rust
#[derive(RustEmbed)]
#[folder = "crates/maestro-web/src/assets/"]
struct Assets;
```

Assets are compiled into the binary, making the Docker image self-contained with no external file dependencies.

---

## Docker

### Multi-Stage Build

```dockerfile
# Stage 1: Build
FROM rust:1.85-bookworm AS builder
WORKDIR /app
COPY . .
RUN cargo build --release

# Stage 2: Runtime
FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y \
    ca-certificates \
    curl \
    git \
    jq \
    iptables \
    nodejs \
    npm \
    && rm -rf /var/lib/apt/lists/*

# Install tools
RUN npm install -g @anthropic-ai/claude-code
# Playwright browsers: supplied per-repo via playwright-cache volume (see README), not baked in image.

# gh CLI (official apt repo)
RUN curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg \
    | dd of=/usr/share/keyrings/githubcli-archive-keyring.gpg \
    && echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" \
    | tee /etc/apt/sources.list.d/github-cli.list > /dev/null \
    && apt-get update && apt-get install -y gh

# acli (download binary directly)
RUN curl -fsSL -o /usr/local/bin/acli <acli-download-url> \
    && chmod +x /usr/local/bin/acli

# figma-cli (npm)
RUN npm install -g figma-cli

# Install Claude Code skills collection
RUN curl -fsSL <skills-install-url> | sh

# Copy binary
COPY --from=builder /app/target/release/maestro /usr/local/bin/maestro
COPY config.toml /etc/maestro/config.toml

EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/maestro"]
```

### Tool Installation Strategy

| Tool | Installation Method | Rationale |
|---|---|---|
| `gh` | Official apt repository | Native Debian package, no Homebrew needed |
| `acli` | Direct binary download | Avoids Homebrew dependency |
| `claude-code` | npm global install | Official distribution method |
| Playwright browsers | per-worktree `playwright install` + `playwright-cache` volume | Matches project lockfile; avoids image/revision skew |
| `figma-cli` | npm global install | Available on npm |
| Skills collection | curl install script | As specified in requirements |

### Egress Allowlist (iptables)

```bash
#!/bin/bash
# egress-rules.sh — applied at container startup

iptables -P OUTPUT DROP
iptables -A OUTPUT -o lo -j ACCEPT
iptables -A OUTPUT -m state --state ESTABLISHED,RELATED -j ACCEPT

# DNS
iptables -A OUTPUT -p udp --dport 53 -j ACCEPT
iptables -A OUTPUT -p tcp --dport 53 -j ACCEPT

# Jira (Atlassian Cloud)
iptables -A OUTPUT -d api.atlassian.com -j ACCEPT
iptables -A OUTPUT -d *.atlassian.net -j ACCEPT

# GitHub
iptables -A OUTPUT -d github.com -j ACCEPT
iptables -A OUTPUT -d api.github.com -j ACCEPT

# Anthropic (Claude API)
iptables -A OUTPUT -d api.anthropic.com -j ACCEPT

# Figma
iptables -A OUTPUT -d api.figma.com -j ACCEPT

# npm registry (for tool installation at build time — not needed at runtime)
# iptables -A OUTPUT -d registry.npmjs.org -j ACCEPT
```

Note: iptables rules use domain names resolved at apply time. For production, resolve to IP ranges or use a forward proxy for dynamic resolution.

The container must run with `--cap-add=NET_ADMIN` to apply iptables rules, or the rules can be baked into the image at build time.

### Docker Compose

```yaml
version: "3.9"
services:
  maestro:
    build: .
    ports:
      - "8080:8080"
    cap_add:
      - NET_ADMIN
    volumes:
      - ./config.toml:/etc/maestro/config.toml:ro
      - gh-auth:/root/.config/gh:ro
      - acli-auth:/root/.config/acli:ro
      - repo:/workspace
    environment:
      - FIGMA_TOKEN=${FIGMA_TOKEN}
      - ANTHROPIC_API_KEY=${ANTHROPIC_API_KEY}
    restart: unless-stopped

volumes:
  gh-auth:
    external: true
  acli-auth:
    external: true
  repo:
```

---

## Authentication Strategy

| Service | Auth Mechanism | Mount/Env |
|---|---|---|
| GitHub (`gh`) | Token in `~/.config/gh/hosts.yml` | Volume mount (read-only) |
| Jira (`acli`) | Token in acli config | Volume mount (read-only) |
| Figma | `FIGMA_TOKEN` environment variable | Docker env var |
| Anthropic (Claude) | `ANTHROPIC_API_KEY` environment variable | Docker env var |

Tokens are never baked into the image. They are injected at runtime via Docker volumes (for file-based auth) or environment variables (for token-based auth).

For production deployments, Docker secrets can replace environment variables:

```yaml
secrets:
  anthropic_key:
    file: ./secrets/anthropic_key.txt
  figma_token:
    file: ./secrets/figma_token.txt
```

---

## Concurrency Model

```
┌─────────────────────────────────────────┐
│              tokio runtime              │
│                                         │
│  ┌──────────┐  ┌──────────────────────┐ │
│  │  Jira    │  │   Web Server         │ │
│  │  Poller  │  │  (axum)              │ │
│  │  (task)  │  │  (task)              │ │
│  └────┬─────┘  └──────────┬───────────┘ │
│       │                   │             │
│       ▼                   ▼             │
│  ┌─────────────────────────────────────┐│
│  │        Workflow Engine              ││
│  │   Arc<RwLock<HashMap<Workflows>>>   ││
│  │                                     ││
│  │  ┌─────────┐ ┌─────────┐ ┌───────┐ ││
│  │  │Workflow 1│ │Workflow 2│ │  ...  │ ││
│  │  │ (task)   │ │ (task)   │ │(task) │ ││
│  │  └─────────┘ └─────────┘ └───────┘ ││
│  └─────────────────────────────────────┘│
│                    │                     │
│                    ▼                     │
│         broadcast::Sender                │
│         (workflow events)                │
│                    │                     │
│         ┌─────────┼──────────┐          │
│         ▼         ▼          ▼          │
│      WS Client  WS Client  WS Client   │
└─────────────────────────────────────────┘
```

### Shared State

- `Arc<RwLock<HashMap<String, Workflow>>>` — workflow map, keyed by ticket key
- `Arc<RwLock<Config>>` — live-reloadable configuration
- `broadcast::Sender<WorkflowEvent>` — event bus for WebSocket push

### Synchronization

- **Read-heavy workload**: `RwLock` allows concurrent reads (dashboard polling, WebSocket updates) with exclusive writes (state transitions).
- **Per-workflow isolation**: Each workflow task only writes to its own `Workflow` struct. The `RwLock` is acquired briefly to update the map entry.
- **No cross-workflow dependencies**: Workflows are independent. No deadlock risk from workflow-to-workflow interaction.
- **Cancellation**: `CancellationToken` per workflow for clean shutdown without holding locks.

---

## Dry Mode

Trait-based abstraction that gates all external side effects:

```rust
#[async_trait]
pub trait ExternalActions: Send + Sync {
    // Jira
    async fn assign_ticket(&self, key: &str) -> Result<()>;
    async fn transition_ticket(&self, key: &str, status: &str) -> Result<()>;
    async fn unassign_ticket(&self, key: &str) -> Result<()>;

    // Git/GitHub
    async fn create_worktree(&self, branch: &str, base: &str) -> Result<PathBuf>;
    async fn remove_worktree(&self, path: &Path) -> Result<()>;
    async fn create_pr(&self, title: &str, body: &str, branch: &str) -> Result<String>;

    // Claude Code
    async fn start_claude_session(&self, worktree: &Path, prompt: &str) -> Result<ClaudeSession>;

    async fn run_command(&self, cmd: &str, cwd: &Path) -> Result<CommandOutput>;
}
```

### RealActions

Executes all operations against real services. Used in production.

### DryRunActions

- **Jira operations**: Logged but not executed. Returns `Ok(())`.
- **Git worktree**: Created locally (this is safe — no remote effect).
- **Claude Code sessions**: Run normally (the coding itself is local).
- **PR creation**: Logged but not executed. Returns a fake PR URL.
- **Other shell hooks**: Run normally when invoked (local-only).

The mode is selected at startup via config:

```toml
[general]
dry_mode = true
```

---

## Configuration

TOML file at `/etc/maestro/config.toml` (configurable via CLI arg):

```toml
[general]
dry_mode = false
poll_interval_secs = 30
max_concurrent_workflows = 5
log_level = "info"

[jira]
project_keys = ["PROJ", "TEAM"]
item_types = ["Task", "Bug"]
jql_filter = ""  # optional additional JQL

[git]
base_branch = "main"
remote = "origin"
repo_path = "/workspace"

[commands]
pre_install = []
install = "npm ci"

[web]
host = "0.0.0.0"
port = 8080

[claude]
skills_path = "/root/.claude/skills"
address_ticket_passes = 3
step_timeout_secs = 600
```

Config is deserialized with serde:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub general: GeneralConfig,
    pub jira: JiraConfig,
    pub git: GitConfig,
    pub commands: CommandsConfig,
    pub web: WebConfig,
    pub claude: ClaudeConfig,
}
```

Config changes via the web UI are:
1. Validated
2. Written to the TOML file
3. Reloaded into the `Arc<RwLock<Config>>`
4. Applied to new workflows (running workflows keep their original config)

---

## Error Handling

### Strategy

- **`thiserror`** for typed error enums per module
- **No panics** in production code — all fallible operations return `Result`
- **Workflow-level recovery**: Errors in a step transition the workflow to `Error` state with the source state preserved for potential retry
- **Step retry**: Lint and test steps retry up to 3 times (run Claude to fix, then re-run)
- **Graceful degradation**: If a single workflow fails, others continue unaffected

### Error Types

```rust
#[derive(Debug, thiserror::Error)]
pub enum MaestroError {
    #[error("Jira error: {0}")]
    Jira(String),

    #[error("Git error: {0}")]
    Git(String),

    #[error("Claude session error: {0}")]
    Claude(String),

    #[error("Command failed: {cmd} (exit code {code})")]
    Command { cmd: String, code: i32, stderr: String },

    #[error("Timeout after {0}s")]
    Timeout(u64),

    #[error("Workflow cancelled")]
    Cancelled,

    #[error("Config error: {0}")]
    Config(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}
```

---

## Logging Strategy

- **`tracing`** crate for structured, async-aware logging
- **Spans** per workflow: every log line within a workflow carries `ticket_key` and `workflow_id` as structured fields
- **Levels**:
  - `ERROR` — workflow failures, process crashes
  - `WARN` — retries, timeouts approaching, non-fatal issues
  - `INFO` — state transitions, step start/completion, PR created
  - `DEBUG` — command output, Claude session events
  - `TRACE` — raw process I/O
- **Output**: JSON format to stdout (container-friendly, parseable by log aggregators)
- **Subscriber**: `tracing_subscriber` with `EnvFilter` for runtime-configurable log levels

```rust
tracing_subscriber::fmt()
    .json()
    .with_env_filter(EnvFilter::from_default_env())
    .with_target(true)
    .with_span_events(FmtSpan::CLOSE)
    .init();
```

Each workflow step is wrapped in a span:

```rust
#[instrument(skip(self, actions), fields(ticket = %workflow.ticket_key, state = ?workflow.state))]
async fn execute_step(&self, workflow: &mut Workflow, actions: &dyn ExternalActions) -> Result<()> {
    // ...
}
```

---

## Jira Polling

```rust
pub struct JiraPoller {
    config: Arc<RwLock<Config>>,
    engine: Arc<WorkflowEngine>,
    actions: Arc<dyn ExternalActions>,
}
```

The poller runs as a long-lived tokio task:

1. Every `poll_interval_secs`, query Jira via `acli` for tickets matching configured project keys and item types in "To Do" status
2. Filter out tickets that already have an active workflow
3. For each new ticket, create a `Workflow` and spawn its task
4. Respects `max_concurrent_workflows` — excess tickets stay in queue

The `acli` command used:

```bash
acli jira issue list --project <KEY> --status "To Do" --type <TYPE> --output json
```

---

## Summary

| Concern | Solution |
|---|---|
| Language | Rust (2024 edition) |
| Async runtime | tokio |
| Web framework | axum |
| Frontend | Vanilla JS + Tailwind CSS (CDN), rust-embed |
| Config | TOML + serde |
| State machine | Enum-based, per-workflow tokio task |
| Process mgmt | tokio::process + CancellationToken |
| Real-time | WebSocket via axum + broadcast channel |
| Dry mode | Trait-based (ExternalActions) |
| Error handling | thiserror + Result propagation |
| Logging | tracing (JSON to stdout) |
| Container | Debian bookworm, multi-stage build |
| Egress control | iptables allowlist |
| Auth | Volume mounts + env vars |
