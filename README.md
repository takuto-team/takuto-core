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
- Claude Code and/or Cursor account (depending on `[agent] provider` in `config.toml`)
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

The image runs `[docker] build_commands` from the TOML file selected at build time (compose build arg **`MAESTRO_BUILD_CONFIG`**, default **`config.toml.example`**). Put skill installers or other one-time setup there, or keep the list empty. To use your real `config.toml` for build hooks, ensure it exists in the build context and set in `docker-compose.yml`:

```yaml
build:
  args:
    MAESTRO_BUILD_CONFIG: config.toml
```

### 3. Setup (first time)

Interactive setup authenticates services and optionally prepares the workspace:

```bash
docker compose run --rm -it maestro setup
```

Use **`-it`** so prompts (GitHub, Atlassian, optional steps) work interactively.

**Podman:** the Compose wrapper does not always allocate a TTY the same way as Docker. Pass stdin/tty explicitly, for example:

```bash
podman compose --podman-run-args="-i -t" run --rm maestro setup
```

If you use the standalone **`podman-compose`** binary instead, see **`podman-compose(1)`** for the equivalent of **`podman run -i -t`** for interactive **`run`** (flags differ by version).

Steps:

1. **GitHub CLI** (required) — OAuth or device code flow
2. **Atlassian CLI** (required) — OAuth or API token (`site` / `email` in `[jira]` for token mode)
3. **Claude Code** (optional) — `claude auth login` (skip with `s` if you use Cursor only)
4. **Cursor Agent** (optional) — `agent login` (skip with `s` if you use Claude only or rely on `CURSOR_API_KEY` in `maestro.env`)
5. **Repository** (optional) — clone or refresh from `[git] repo_url` into `/workspace` (skip with `s` if you manage the workspace yourself)

Custom Claude skills are **not** installed automatically. Add install commands to **`[docker] build_commands`** and/or **`compose_up_commands`** in `config.toml`, and point **`MAESTRO_BUILD_CONFIG`** at that file when building if hooks should run at image build time.

On every **`docker compose up`**, the entrypoint runs **`maestro preflight`** (GitHub + Atlassian + provider-specific auth), then **`[docker] compose_up_commands`**, then starts the server.

Auth state persists in Docker volumes across container restarts (including `cursor-auth` for Cursor Agent when using interactive login).

### 4. AWS Credentials (if using CodeArtifact)

If your project uses a private npm registry (e.g., AWS CodeArtifact), copy your AWS credentials:

```bash
podman run --rm -v maestro_aws-config:/data -v ~/.aws:/src:ro alpine cp -r /src/. /data/
```

Then configure `pre_install` in `config.toml` (an array of shell commands, run in order):

```toml
[commands]
pre_install = [
  "aws codeartifact login --tool npm --repository REPO --domain DOMAIN --domain-owner OWNER_ID",
]
```

For a single command you can still use a string (backward compatible):

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
| `pre_install` | `[]` | Shell commands to run in order before install (e.g., registry auth); a single string is accepted for backward compatibility |
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
| `model` | `""` | Model override for Claude Code when non-empty |

### `[agent]`

| Key | Default | Description |
|-----|---------|-------------|
| `provider` | `"claude"` | `claude` (Claude Code CLI) or `cursor` (Cursor Agent CLI) for implement / review / fix / PM steps |
| `cursor_cli` | `"agent"` | Executable name or path for Cursor Agent (see [Cursor CLI](https://cursor.com/docs/cli/overview)); only used when `provider = "cursor"` |
| `cursor_model` | `"Auto"` | Cursor Agent `--model`; `Auto` (any case) omits the flag so Cursor picks the model |

The image includes the Cursor Agent CLI (`agent` in `/usr/local/bin`). Run `docker compose run --rm -it maestro setup` and complete the Cursor step, or set **`CURSOR_API_KEY`** in `maestro.env` (recommended for unattended / non-TTY `docker compose up`). **`docker-compose.yml`** sets **`CURSOR_CONFIG_DIR=/home/maestro/.cursor`** so browser login matches the **`cursor-auth`** volume; without that, tokens can land under **`~/.config/cursor`** and look “missing” on the next start. Ensure egress allows Cursor’s API hosts if you use a firewall.

### `[docker]`

| Key | Default | Description |
|-----|---------|-------------|
| `build_commands` | `[]` | Shell commands (`bash -c`) run **once** during `docker compose build` (config file chosen by build arg `MAESTRO_BUILD_CONFIG`) |
| `compose_up_commands` | `[]` | Shell commands run on **every** `docker compose up` after auth preflight, before the server |

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
| `cursor-auth` | `/home/maestro/.cursor` | Cursor Agent data (when using interactive `agent login`) |
| `gh-auth` | `/home/maestro/.config/gh` | GitHub CLI auth |
| `acli-auth` | `/home/maestro/.config/acli` | Atlassian CLI auth |
| `workspace` | `/workspace` | Cloned repository |
| `npm-cache` | `/home/maestro/.npm` | npm download cache |
| `aws-config` | `/home/maestro/.aws` | AWS credentials (optional) |

**Skills and `compose_up_commands`:** Claude/Cursor skills must be written **inside the container** under **`/home/maestro/.claude`** and **`/home/maestro/.cursor`** (the named volumes above). They do **not** appear in your project folder on the host. If a hook uses **`$HOME/.claude/skills`** but **`HOME`** is empty (seen with some Podman setups), files can end up under **`/.claude/...`** on the writable layer and **disappear** on the next recreate — use **`MAESTRO_HOME`** / absolute paths. Compose sets **`MAESTRO_HOME=/home/maestro`** and **`maestro docker-hooks`** passes **`HOME`**, **`MAESTRO_HOME`**, and **`CURSOR_CONFIG_DIR`** into each hook. Verify after `up`:

```bash
docker compose exec maestro ls -la /home/maestro/.claude/skills
docker compose exec maestro ls -la /home/maestro/.cursor/skills
```

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
docker compose run --rm -it maestro setup
```

### Cursor `agent login`: `bad option: --use-system-ca`

The Cursor CLI runs Node with **`--use-system-ca`**, which only exists on **Node.js ≥ 23.9** (Linux). The Maestro image ships **Node 23** from **nodejs.org** for that reason. **Rebuild** the image (`docker compose build` / `podman compose build`) so you are not on an older layer that used Node 20.

### Cursor `agent login`: `/usr/local/bin/node: No such file or directory`

The `agent` wrapper expects **`node`** next to it on **`PATH`** (typically `/usr/local/bin/node`). The current image installs the official Node tarball into **`/usr/local`**, so this usually means the image is outdated or the binary was removed — **rebuild** the image.

### Cursor `agent login`: `Cannot find module '/usr/local/bin/index.js'`

The Cursor install script puts **`agent`** next to **`index.js`** and a bundled **`node`** under **`~/.local/share/cursor-agent/versions/...`**. Copying only the launcher script to **`/usr/local/bin/agent`** makes it resolve **`index.js`** relative to **`/usr/local/bin`**, where that file does not exist. Current Dockerfiles copy the full **`cursor-agent`** tree to **`/usr/local/share/cursor-agent`** and symlink **`/usr/local/bin/agent`** to the real launcher — **rebuild** the image (`docker compose build` / `podman compose build --no-cache`).

### Project tool versions (`mise`)

The image installs **[mise](https://mise.jdx.dev/)** from the official apt repository, plus **`build-essential`**, **`libssl-dev`**, and related headers so **`mise install`** can compile runtimes such as **Ruby** (ruby-build builds OpenSSL, then Ruby) when no prebuilt binary exists—common on **arm64**. Repositories can pin Node, Python, and other tools with **`.mise.toml`**, **`mise.toml`**, **`.tool-versions`**, or **`.config/mise/config.toml`**. Maestro runs **`mise install`** in the worktree when such a file is present, then runs **`[commands]`** shell steps through **`mise exec`** so those versions apply. Default **Node 23** in **`/usr/local`** remains for the Cursor **`agent`** wrapper; project Node from mise is used inside **`mise exec`** (and via shims on **`PATH`**). Tool installs persist in the **`mise-data`** and **`mise-cache`** volumes.

### `docker compose up` stalls after “Egress rules applied”

The entrypoint then switches to the **`maestro`** user and runs **auth preflight** (`gh`, `acli`, and optionally **`agent status`** or **`claude auth status`**). A hang here is often **`su -`** waiting on a TTY under Podman, or **`agent status`** blocking without a TTY / leaving child processes alive. The image uses **`runuser`** (not a login **`su -`**), preflight logs each step (`[maestro preflight] …`), **`agent status`** has a **45s** timeout and kills the **process group**, and Cursor skips **`agent status`** when **`CURSOR_API_KEY`** is set or when **`cli-config.json`** (under **`CURSOR_CONFIG_DIR`**) already contains token-like fields. Rebuild the image so **`maestro` is current**. If Cursor still says not authenticated inside the container, re-run **`agent login`** once after upgrading (so tokens are written under **`CURSOR_CONFIG_DIR`**) or set **`CURSOR_API_KEY`**. Large **`compose_up_commands`** downloads may take minutes — you should see **`[maestro] Running docker startup hooks...`** before the hook output.

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
