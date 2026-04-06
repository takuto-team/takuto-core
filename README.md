# Maestro

Automated Jira ticket handler that drives **Claude Code** or **Cursor Agent** in headless mode. Picks up tickets from Jira, creates branches, runs optional install hooks, runs configurable **`[[agent_steps]]`** prompts (implementation, review, lint/tests, **`gh` PR creation**, or anything else). Maestro does **not** run a built-in PR step: the workflow completes when the agent sequence finishes; optional PR URLs for the dashboard come from **`.maestro/outcome.toml`** or a **`MAESTRO_PR_URL:`** line in agent output (see headless instructions in the engine). All inside an isolated Docker container with a real-time monitoring dashboard.


## Security and operations

- **Untrusted Jira text** (descriptions, linked issues) is embedded in AI prompts as **`{ticket_context}`**. Treat it like user-supplied content: use **Jira permissions**, **branch protection**, and **human code review**. Maestro adds explicit **UNTRUSTED_JIRA** framing and optional **`[jira]`** limits (`linked_items_in_prompt`, byte caps); that **reduces** prompt-injection risk but does not remove it.
- **`acli`** invocations are **allowlisted** to Jira workitem read/search/assign/transition (plus `jira auth status` in preflight). Extend only with **`[jira] acli_allowed_extra_prefixes`** if you understand the risk.
- **Dashboard `PUT /api/config`** only accepts **`web`** (login) and **`general.max_concurrent_workflows`** / **`max_active_workflows`** — **strict JSON**; anything else returns **400**. Change Jira, git, agent steps, install commands, etc. in **`config.toml`** and **restart** Maestro.


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

**Podman on macOS:** the default Podman machine has 2 CPUs and 4GB RAM, which is too low — agent workflows (linting, tests) will OOM. Increase before first use:

```bash
podman machine stop
podman machine set --memory 12288 --cpus 4
podman machine start
```

## Quick Start

### 1. Configure

```bash
cp config.toml.example config.toml
cp maestro.env.example maestro.env
cp .env.example .env   # optional — Compose reads `.env` for FIGMA_API_TOKEN, COMPOSE_FILE, etc.
```

Edit `config.toml` with your project settings (see [Configuration](#configuration) below).

Edit `maestro.env` with any custom environment variables needed inside the container (e.g., API keys, custom base URLs).

Edit `.env` if you use **Compose-only** variables (see **`.env.example`**) such as **`FIGMA_API_TOKEN`**.

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

Optional **Claude/Cursor skills** from a gitignored **`./skills`** folder at the **Maestro repo root** are merged into the container home volumes on **every** start (see **Project skills** under Docker Volumes). For anything else, add **`[docker] build_commands`** / **`compose_up_commands`** in `config.toml` and point **`MAESTRO_BUILD_CONFIG`** at that file when building.

On every **`docker compose up`**, the entrypoint merges **`./skills`**, runs **`maestro preflight`** (GitHub + Atlassian + provider-specific auth), then **`[docker] compose_up_commands`**, then starts the server.

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

### Docker-in-Docker sidecar (optional)

To run **`docker`** inside Maestro (for example nested **`docker run`**, `docker compose up`, or Playwright containers), merge the DinD sidecar Compose file. This runs a real Docker daemon in a sidecar container; Maestro connects via `DOCKER_HOST=tcp://dind:2375` automatically. Works on **all platforms** (macOS Podman, macOS Docker Desktop, Linux).

> **`podman-compose` (Python) caveat**: `podman-compose` does **not** read `COMPOSE_FILE` from **`.env`**. You must pass **`-f`** flags explicitly:
> ```bash
> podman compose -f docker-compose.yml -f docker-compose.dind.yml up -d
> ```
> `docker compose` (Go plugin) reads `COMPOSE_FILE` from `.env` as expected.

The image installs Debian’s **`docker.io`** package for the **`docker`** CLI (no in-container daemon). The DinD sidecar provides the daemon.

**Workflow isolation:** when DinD is available, Maestro automatically runs each workflow’s install and agent steps in **ephemeral Docker containers** via the DinD daemon. This prevents port conflicts and filesystem side-effects between concurrent workflows. After `make up`, load the Maestro image into DinD so worker containers can start:

```bash
make load-worker
```

If the worker image is not loaded, the entrypoint logs a warning and falls back to local execution (no isolation).

**Security:** the DinD sidecar runs **`--privileged`** but is isolated to its own container — Maestro itself gains no extra privileges. The `workspace` volume is shared so paths resolve identically between both containers.

**Playwright / visual regression tests:** isolated workers use the **same Chromium revision as your repo’s `@playwright/test`** (downloaded into a persisted **`playwright-cache`** volume under `~/.cache/ms-playwright`), not a separate browser bundled in the Maestro image — that mismatch used to cause subtle pixel drift vs `npm run …` on your laptop or CI. Workers also default to **`TZ=UTC`** and **`LANG`/`LC_ALL=C.UTF-8`** for more stable screenshots. Remaining differences vs macOS (font rasterization, subpixel AA) can still appear if baselines were captured on macOS while Maestro runs Linux; prefer generating baselines in the same environment as CI (often Linux), or set **`[general] worker_image`** to an image that matches your visual-test stack.

**After changing compose files**, recreate containers: `podman compose -f docker-compose.yml -f docker-compose.dind.yml up -d --force-recreate`.

### Dry Mode

Set `dry_mode = true` in `config.toml` to run the full workflow without Maestro’s **Jira/GitHub trait** side effects (no ticket assignment, no status changes, no `ExternalActions::create_pr` — which the engine does not call anyway). Local operations (worktree creation, npm install, agent sessions) still execute; an agent can still run **`gh`** in the worktree unless you constrain it.

## Configuration

All configuration is in `config.toml` (see `config.toml.example` for defaults).

### Root-level `[[agent_steps]]` (TOML)

Optional **`[[agent_steps]]`** tables belong at the **root** of the file. In TOML, tables that appear *after* a `[section]` can bind incorrectly — place **`[[agent_steps]]`** **before** `[general]` (see `config.toml.example`).

| Key | Default | Description |
|-----|---------|-------------|
| `[[agent_steps]]` | *(built-in)* | Each entry: `name`, `prompt` (placeholders: `ticket_key`, `ticket_summary`, `ticket_description`, `ticket_type`, `acceptance_criteria`, `ticket_context`), **`repeat`** (default `1` — run this step this many times in a row with session resume). **Any** custom step replaces the entire built-in list; omit all `[[agent_steps]]` for generic built-in prompts |
| *(no custom steps)* | — | Built-in sequence runs **`[claude] address_ticket_passes`** times (default `3`) |

### `[general]`

| Key | Default | Description |
|-----|---------|-------------|
| `dry_mode` | `false` | Run without Jira/GitHub writes |
| `poll_interval_secs` | `60` | Seconds between Jira polls |
| `max_concurrent_workflows` | `1` | Max parallel ticket workflows |
| `log_level` | `"info"` | Log level: trace, debug, info, warn, error |
| `worker_image` | `""` | Docker image for isolated workflow containers; empty = auto-detect from running Maestro container |

### `[jira]`

| Key | Default | Description |
|-----|---------|-------------|
| `project_keys` | `[]` | Jira project keys to poll (e.g., `["PROJ"]`) |
| `item_types` | `["Task", "Bug"]` | Ticket types to handle |
| `jql_filter` | `""` | Additional JQL filter |
| `site` | `""` | Jira site host or base URL (e.g., `"company.atlassian.net"`) — token auth, egress rules, and ticket context for prompts (empty → `jira.atlassian.net` where the code needs a default host) |
| `email` | `""` | Jira user email — used for token auth |

### `[git]`

| Key | Default | Description |
|-----|---------|-------------|
| `base_branch` | `"main"` | Branch to create worktrees from |
| `remote` | `"origin"` | Git remote name for fetch, worktree base ref, and push |
| `repo_url` | `""` | Git repository URL (cloned during setup) |
| `repo_path` | `"/workspace"` | Path inside container |

### `[commands]`

| Key | Default | Description |
|-----|---------|-------------|
| `pre_install` | `[]` | Shell commands to run in order before install (e.g., registry auth); a single string is accepted for backward compatibility |
| `install` | `""` | Dependency install command (e.g., `"npm ci"`) |

### `[web]`

| Key | Default | Description |
|-----|---------|-------------|
| `host` | `"0.0.0.0"` | Web server bind address |
| `port` | `8080` | Web server port |

### `[claude]`

| Key | Default | Description |
|-----|---------|-------------|
| `address_ticket_passes` | `3` | When **`[[agent_steps]]`** is empty: how many times to run the full built-in two-step sequence. Ignored for counting when you define custom **`[[agent_steps]]`** (use per-step **`repeat`** instead) |
| `step_timeout_secs` | `1800` | Timeout per Claude session (30 min) |
| `figma_api_token` | `""` | Figma API token for design references |
| `model` | `""` | Model override for Claude Code when non-empty |

### `[agent]`

| Key | Default | Description |
|-----|---------|-------------|
| `provider` | `"claude"` | `claude` (Claude Code CLI) or `cursor` (Cursor Agent CLI) for agent steps and PM validation |
| `cursor_cli` | `"agent"` | Executable name or path for Cursor Agent (see [Cursor CLI](https://cursor.com/docs/cli/overview)); only used when `provider = "cursor"` |
| `cursor_model` | `"Auto"` | Cursor Agent `--model`; `Auto` (any case) or empty uses automatic model selection |

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
6. **Agent steps** — built-in or custom **`[[agent_steps]]`**: each step is a headless Claude/Cursor session (prompts can include “run `npm run lint` and fix issues”, tests, review, etc.). With no custom steps, the default two-step sequence repeats **`[claude] address_ticket_passes`** times (default `3`).
7. **Workflow complete** — engine records optional **`pr_url`** from **`.maestro/outcome.toml`** or **`MAESTRO_PR_URL:`**; fails earlier if any logged step **Failed**

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

### Project skills (`./skills`)

Add a **`skills`** directory at the **root of the Maestro project** (same level as `docker-compose.yml`). It is **gitignored** so each developer or CI can use different skill sets without committing them.

- **`docker compose build`:** If **`./skills`** exists and is non-empty in the build context, its contents are **baked** into the image under **`/opt/maestro/project-skills-baked`** (BuildKit `RUN --mount=type=bind`). Requires **BuildKit** (`DOCKER_BUILDKIT=1` for Docker; Podman 4+ supports the same Dockerfile syntax).
- **`docker compose up`:** Compose bind-mounts **`./skills`** read-only to **`/opt/maestro/project-skills-host`**. If the host path is missing, the engine typically creates an **empty** directory; the merge step is then a no-op for the host layer.

On **each container start** (as **root**, before switching to **`maestro`**), **`merge-project-skills.sh`** copies each **top-level** entry from:

1. **`/opt/maestro/project-skills-baked`** (image), then  
2. **`/opt/maestro/project-skills-host`** (host **`./skills`**),

into all of:

- **`/home/maestro/.claude/skills`**
- **`/home/maestro/.cursor/skills`**
- **`/home/maestro/.cursor/skills-cursor`**

**Precedence:** For a given skill **name**, the **host `./skills`** copy **overwrites** the baked copy. Anything already on the named volumes whose name is **not** in those layers is **left unchanged**. Replacing a name removes the old tree under that name on all three destinations, then copies the project version.

Verify after `up`:

```bash
docker compose exec maestro ls -la /home/maestro/.claude/skills
docker compose exec maestro ls -la /home/maestro/.cursor/skills
```

**Do not confuse** **`/workspace/.cursor`** (files from your **cloned app repo**: rules, commands, etc.) with **`/home/maestro/.cursor`** on the **`cursor-auth`** volume. Project skills land under **`/home/maestro/.cursor/skills`**, not next to **`/workspace/.cursor/rules`**.

**`compose_up_commands`:** Optional extra steps run as **`maestro`** after preflight. If a hook writes under **`~/.claude`**, ensure **`HOME`** / **`MAESTRO_HOME`** are set (Compose sets them; **`maestro docker-hooks`** passes **`HOME`**, **`MAESTRO_HOME`**, **`CURSOR_CONFIG_DIR`**).

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

### Podman on Linux with SELinux

Named volumes can get **MCS labels** that block both **`maestro` and root-in-container** from listing or removing files under **`~/.claude/skills`**, which breaks skill-sync hooks (`rm: Permission denied` even when the sync script logs `uid=0`). **`docker-compose.yml`** sets **`security_opt: [label=disable]`** so the container is not SELinux-confined; Docker Desktop and hosts without SELinux ignore this. If you must keep labeling, relabel the volume from the host (for example bind mounts with **`:z`** / **`:Z`**) instead of removing **`label=disable`**.

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
