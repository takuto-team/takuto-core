# Maestro

Automated Jira ticket handler that uses Claude Code in headless mode. Picks up tickets from Jira, creates branches, implements changes via AI, runs linting/tests, and creates pull requests — all inside an isolated Docker container with a real-time monitoring dashboard.

## Architecture

- **Rust backend** (3-crate workspace): workflow orchestrator, web server, CLI
- **Web dashboard**: real-time terminal streaming via WebSocket, workflow cards, configuration page
- **Docker container**: allowlist-only egress, non-root execution, persistent auth volumes

## Prerequisites

- Docker or Podman with Compose
- GitHub account (for `gh` CLI auth)
- Jira/Atlassian account (for `acli` CLI auth)
- Claude Code account (for AI sessions)
- AWS credentials (only if using CodeArtifact for npm registry)

## Quick Start

### 1. Configure

```bash
cp config.toml.example config.toml
cp maestro.env.example maestro.env
```

Edit `config.toml` with your project settings (see [Configuration](#configuration) below).

Edit `maestro.env` with any custom environment variables needed inside the container (e.g., API keys, custom base URLs).

### 2. Build

```bash
docker compose build
```

### 3. Setup (first time)

Interactive setup authenticates all services and clones your repository:

```bash
docker compose run maestro setup
```

This walks through 5 steps:

1. **Claude Code** — OAuth via browser (copy the URL to your host browser)
2. **GitHub CLI** — OAuth or device code flow
3. **Atlassian CLI** — OAuth or API token
4. **Repository** — Clones the repo URL from `config.toml` into `/workspace`
5. **Skills** — Installs Claude Code skills for ticket handling

Auth state persists in Docker volumes across container restarts.

### 4. AWS Credentials (if using CodeArtifact)

If your project uses a private npm registry (e.g., AWS CodeArtifact), copy your AWS credentials:

```bash
podman run --rm -v maestro_aws-config:/data -v ~/.aws:/src:ro alpine cp -r /src/. /data/
```

Then configure the `pre_install` command in `config.toml`:

```toml
[commands]
pre_install = "aws codeartifact login --tool npm --repository REPO --domain DOMAIN --domain-owner OWNER_ID"
```

And add the registry domain to the egress allowlist:

```toml
[network]
extra_egress_hosts = ["yourcompany-123456.d.codeartifact.region.amazonaws.com"]
```

### 5. Start

```bash
docker compose up
```

Dashboard at **http://localhost:8080**.

### Dry Mode

Set `dry_mode = true` in `config.toml` to run the full workflow without writing to Jira or GitHub (no ticket assignment, no status changes, no PR creation). Local operations (worktree creation, npm install, Claude sessions, linting, tests) still execute.

## Configuration

All configuration is in `config.toml` (see `config.toml.example` for defaults).

### `[general]`

| Key | Default | Description |
|-----|---------|-------------|
| `dry_mode` | `false` | Run without Jira/GitHub writes |
| `poll_interval_secs` | `60` | Seconds between Jira polls |
| `max_concurrent_workflows` | `1` | Max parallel ticket workflows |
| `max_fix_attempts` | `3` | Max retries for lint/test fix loops |
| `log_level` | `"info"` | Log level: trace, debug, info, warn, error |

### `[jira]`

| Key | Default | Description |
|-----|---------|-------------|
| `project_keys` | `[]` | Jira project keys to poll (e.g., `["PROJ"]`) |
| `item_types` | `["Task", "Bug"]` | Ticket types to handle |
| `jql_filter` | `""` | Additional JQL filter |
| `site` | `""` | Jira site (e.g., `"company.atlassian.net"`) — used for token auth and egress rules |
| `email` | `""` | Jira user email — used for token auth |

### `[git]`

| Key | Default | Description |
|-----|---------|-------------|
| `base_branch` | `"main"` | Branch to create worktrees from |
| `repo_url` | `""` | Git repository URL (cloned during setup) |
| `repo_path` | `"/workspace"` | Path inside container |

### `[commands]`

| Key | Default | Description |
|-----|---------|-------------|
| `pre_install` | `""` | Command to run before install (e.g., registry auth) |
| `install` | `""` | Dependency install command (e.g., `"npm ci"`) |
| `lint` | `""` | Linting command (e.g., `"npm run lint"`) |
| `unit_test` | `""` | Unit test command (e.g., `"npm test"`) |
| `e2e_test` | `""` | E2E test command (e.g., `"npm run test:e2e"`) |

### `[web]`

| Key | Default | Description |
|-----|---------|-------------|
| `host` | `"0.0.0.0"` | Web server bind address |
| `port` | `8080` | Web server port |

### `[claude]`

| Key | Default | Description |
|-----|---------|-------------|
| `address_ticket_passes` | `3` | Number of address-ticket + review rounds |
| `step_timeout_secs` | `1800` | Timeout per Claude session (30 min) |
| `figma_api_token` | `""` | Figma API token for design references |
| `model` | `""` | Model override; also used when `[agent] provider = "cursor"` |

### `[agent]`

| Key | Default | Description |
|-----|---------|-------------|
| `provider` | `"claude"` | `claude` (Claude Code CLI) or `cursor` (Cursor Agent CLI) for implement / review / fix / PM steps |
| `cursor_cli` | `"agent"` | Executable name or path for Cursor Agent (see [Cursor CLI](https://cursor.com/docs/cli/overview)); only used when `provider = "cursor"` |

For Cursor in a container, install the CLI (`curl … \| bash` from Cursor docs), run `agent login` or set `CURSOR_API_KEY`, and ensure egress allows Cursor’s API hosts if you use a firewall.

### `[network]`

| Key | Default | Description |
|-----|---------|-------------|
| `extra_egress_hosts` | `[]` | Additional domains to allow through the egress firewall |

## Workflow

For each ticket in "To Do" status:

1. **Assign** ticket to the logged-in user, move to "In Progress"
2. **Retrieve** ticket details and linked items from Jira
3. **Create worktree** on a new branch from the base branch
4. **Pre-install** (optional) — run registry auth or other setup
5. **Install dependencies** — e.g., `npm ci`
6. **Address ticket** (3 passes) — Claude Code implements the ticket using the `/address-ticket` skill
7. **Review changes** (3 passes) — Claude Code reviews using `/review-changes`
8. **Lint** — run linting, fix errors via Claude, repeat until clean
9. **Unit tests** — run tests, fix failures via Claude, repeat until passing
10. **E2E tests** — run e2e tests, fix failures via Claude, repeat until passing
11. **Create PR** — conventional commit title, Jira reference in description

On **stop**: kills running sessions, unassigns ticket, moves back to "To Do".

## Dashboard

The web dashboard at `http://localhost:8080` provides:

- **Workflow cards** — 2 per row, showing ticket, status, current step, progress bar
- **Real-time terminal** — live streaming of command output via WebSocket
- **Controls** — Pause, Resume, Stop, Retry buttons
- **Report modal** — detailed step-by-step execution report
- **Configuration page** — at `/config.html`

Terminal output persists across page reloads (last 100 lines served via API).

## Environment Variables

Custom environment variables can be set in `maestro.env` (mounted at `/etc/maestro/env`). This file is sourced at container startup for both setup and normal mode.

```bash
cp maestro.env.example maestro.env
```

Example `maestro.env`:
```bash
export ANTHROPIC_BASE_URL="https://custom-proxy.example.com/claude"
export CLAUDE_CODE_OAUTH_TOKEN="your-token"
```

**Note:** Only use `export VAR=value` syntax. Aliases and commands are not supported in this file. Use `pre_install` in `config.toml` for commands that need to run before the workflow.

## Container Details

### Egress Allowlist

The container uses iptables to restrict outbound traffic. Allowed by default:

- Jira/Atlassian (api.atlassian.com + your configured `site`)
- GitHub (github.com, api.github.com, raw.githubusercontent.com)
- Anthropic/Claude (api.anthropic.com, api.claude.ai, claude.ai, console.anthropic.com)
- npm registry (registry.npmjs.org)
- Private registries detected from `.npmrc` files
- Custom hosts from `[network] extra_egress_hosts`

### Docker Volumes

| Volume | Mount Point | Purpose |
|--------|-------------|---------|
| `claude-auth` | `/home/maestro/.claude` | Claude Code auth + skills |
| `gh-auth` | `/home/maestro/.config/gh` | GitHub CLI auth |
| `acli-auth` | `/home/maestro/.config/acli` | Atlassian CLI auth |
| `workspace` | `/workspace` | Cloned repository |
| `npm-cache` | `/home/maestro/.npm` | npm download cache |
| `aws-config` | `/home/maestro/.aws` | AWS credentials (optional) |

### Non-root Execution

The container starts as root (for iptables), then switches to the `maestro` user. Claude Code requires non-root execution for `--allow-dangerously-skip-permissions`.

### Logs

Per-workflow log files are written to `/workspace/logs/<TICKET-KEY>.log` with timestamped entries. View from host:

```bash
podman exec <container> cat /workspace/logs/NERO-176.log
```

## Troubleshooting

### "Exit handler never called" during npm ci

Your npm registry is blocked by egress rules. Check the npm debug log:
```bash
podman exec -u maestro <container> tail -30 /home/maestro/.npm/_logs/*-debug-0.log
```
Add the registry domain to `[network] extra_egress_hosts` in `config.toml`.

### Claude Code "api_retry error: unknown"

The Anthropic API endpoint is blocked. Ensure `api.claude.ai` is reachable:
```bash
podman exec -u maestro <container> curl -s -o /dev/null -w "%{http_code}" https://api.claude.ai
```
If blocked, check egress rules. You may need additional domains in `extra_egress_hosts`.

### Auth not found after rebuild

Auth is stored in Docker volumes. If volumes were deleted, re-run setup:
```bash
docker compose run maestro setup
```

### Container name issues with Podman

Podman-compose may leave orphaned containers. Clean up:
```bash
podman stop -a && podman rm -f $(podman ps -aq) 2>/dev/null
podman pod rm -f $(podman pod ls -q) 2>/dev/null
```

## Development

### Build locally

```bash
cargo build
cargo test
cargo check
```

### Project structure

```
crates/
  maestro-core/    # Workflow engine, Jira/GitHub/Claude integrations, config
  maestro-web/     # Axum web server, REST API, WebSocket, static assets
  maestro-cli/     # CLI entry point
docker/
  entrypoint.sh    # Container entrypoint (root preamble + maestro user)
  egress-rules.sh  # iptables egress allowlist
  test-*.sh        # Diagnostic test scripts
design/            # HTML/CSS mockups
```
