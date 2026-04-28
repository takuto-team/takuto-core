# Maestro

**Maestro is an autonomous AI coding pipeline.** It picks up tickets from Jira or GitHub Issues, writes the code, runs your tests and linter, opens a PR — all unattended, inside a secure Docker container, while you focus on what matters.

---

## What you can achieve

- **Go from ticket to PR automatically** — Maestro reads your Jira or GitHub Issues backlog, grabs tickets in "To Do", and drives an AI agent (Claude Code or Cursor) through a fully configurable pipeline: worktree → install → implement → lint/tests → PR.
- **Run multiple tickets in parallel** — configure how many workflows run concurrently; each gets its own git worktree and isolated environment.
- **Monitor everything in real time** — a live web dashboard streams terminal output per workflow, shows progress, and lets you pause, resume, retry, or inspect any run.
- **Jump into any workflow** — open a browser-based VS Code editor and web terminal, pre-configured with your project tools, pointed at the exact worktree the agent is working on.
- **Define your own pipeline steps** — TOML workflow definitions let you chain phases: implement → address PR comments → merge base branch → deploy. Steps depend on each other; trigger them from the dashboard.
- **Work without a ticketing system** — paste any description via the dashboard and Maestro treats it as a workflow. No Jira account required.

---

## Why Maestro?

| | IDE assistant (Copilot, Cursor inline) | Maestro |
|---|---|---|
| **Where it runs** | Inside your editor, on your machine | Inside Docker, on any machine or server |
| **Supervision required** | Yes — you approve each step | No — runs unattended overnight |
| **Ticketing integration** | None | Jira, GitHub Issues, or standalone |
| **Pipeline definition** | Single prompt | Multi-step TOML: implement, review, test, PR, deploy |
| **Concurrent work** | One task at a time | Multiple tickets in parallel |
| **Security boundary** | Full internet access from agent | Egress firewall — only approved hosts reachable |
| **Team deployment** | Per-developer only | Self-host on a server; shared dashboard |
| **Persistence** | Session ends when you close your editor | Survives container restarts; paused workflows resume |

---

## Quick start — Individual developer (local)

Run Maestro on your laptop. Takes about 10 minutes.

### 1. Configure

```bash
cp config.toml.example config.toml
cp maestro.env.example maestro.env
```

Edit `config.toml`:
- Set `[git] repo_url` to your repository
- Set `[general] ticketing_system` to `"jira"`, `"github"`, or `"none"`
- For Jira: fill in `[jira] site`, `project_keys`, `email`
- For GitHub Issues: the repo in `[git] repo_url` is used automatically

### 2. Build

```bash
docker compose build
```

### 3. Authenticate (first time only)

```bash
docker compose run --rm -it maestro setup
```

Walks you through: GitHub CLI → Atlassian CLI (optional) → Claude Code or Cursor Agent → clone your repo.

**Podman on macOS:** increase the default machine resources first:
```bash
podman machine stop && podman machine set --memory 12288 --cpus 4 && podman machine start
```

### 4. Start

```bash
docker compose up
```

Dashboard at **http://localhost:8080**.

If you use Jira or GitHub Issues, Maestro starts polling automatically. Otherwise, click **+** to paste a description and kick off a workflow manually.

---

## Quick start — Teams (server deployment)

Self-host Maestro on a Linux server. Your team accesses the dashboard through a browser; the agent runs in the background.

### 1. Clone and configure on the server

```bash
git clone <maestro-repo-url> && cd maestro-core
cp config.toml.example config.toml
cp maestro.env.example maestro.env
```

Key settings for server deployments:

```toml
[general]
ticketing_system = "jira"          # or "github"
max_concurrent_workflows = 3       # tune to your server's CPU/RAM

[web]
host = "0.0.0.0"
port = 8080

[web.login]
username = "admin"
password = "choose-a-strong-password"
```

Put secrets in `maestro.env` (never in `config.toml`):
```bash
export ANTHROPIC_API_KEY="sk-ant-..."
export GH_TOKEN="github_pat_..."
```

### 2. Build and authenticate

```bash
docker compose build
docker compose run --rm -it maestro setup
```

### 3. Start as a service

```bash
docker compose up -d
```

### 4. Expose with a reverse proxy (recommended)

Point nginx or Caddy at port 8080 and terminate TLS there. Maestro listens on plain HTTP; put HTTPS termination in front.

Example Caddy snippet:
```
maestro.yourcompany.com {
    reverse_proxy localhost:8080
}
```

### Persistence across restarts

All auth state (GitHub, Atlassian, Claude/Cursor), workflow snapshots, and npm cache live in named Docker volumes. Workflows survive `docker compose restart` — paused or in-progress runs resume automatically.

---

## How a workflow runs

For each ticket (or manual entry):

1. **Assign** — ticket moved to "In Progress" in Jira / GitHub *(skipped in no-ticketing mode)*
2. **Worktree** — new git branch from your configured base branch
3. **Install** — runs `pre_install` commands then `install` (e.g. `npm ci`) in the worktree
4. **Agent steps** — one or more AI sessions using your TOML workflow definition; each step has a prompt with ticket context injected
5. **Done** — dashboard shows PR URL (from `.maestro/outcome.toml` or `MAESTRO_PR_URL:` in agent output)

On **stop**: active sessions killed, ticket reverted to "To Do".

### Dynamic workflow definitions

Drop `*.toml` files in the `workflows/` directory to define your pipeline. Start from the examples:

```bash
cp workflows/implement_ticket.example.toml workflows/implement_ticket.toml
```

Each definition has `[[steps]]` with a `prompt` (ticket context auto-injected) or `commands`. Chain definitions with `depends_on` — a "merge base" step only becomes available after "implement ticket" completes.

---

## Configuration reference

Full configuration in `config.toml`. See `config.toml.example` for annotated defaults.

### Root-level `[[agent_steps]]` *(deprecated — use workflow definitions instead)*

| Key | Default | Description |
|-----|---------|-------------|
| `[[agent_steps]]` | *(built-in)* | Legacy inline steps. Each entry: `name`, `prompt` (placeholders: `ticket_key`, `ticket_summary`, `ticket_description`, `ticket_type`, `acceptance_criteria`, `ticket_context`), `repeat`, `when` (`always`/`ticketing`/`no_ticketing`), optional `skills`, optional `resume_previous` |

### `[general]`

| Key | Default | Description |
|-----|---------|-------------|
| `ticketing_system` | `"none"` | `"jira"`, `"github"`, or `"none"` |
| `dry_mode` | `false` | Run without Jira/GitHub writes |
| `poll_interval_secs` | `60` | Seconds between Jira/GitHub polls |
| `max_concurrent_workflows` | `1` | Max parallel workflows |
| `log_level` | `"info"` | `trace`, `debug`, `info`, `warn`, `error` |
| `worker_image` | `""` | Docker image for isolated workflow containers; empty = auto-detect |

### `[jira]`

| Key | Default | Description |
|-----|---------|-------------|
| `project_keys` | `[]` | Jira project keys to poll (e.g. `["PROJ"]`) |
| `item_types` | `["Task", "Bug"]` | Ticket types to handle |
| `jql_filter` | `""` | Extra JQL AND-merged into the dashboard ticket search |
| `site` | `""` | Jira site host (e.g. `"company.atlassian.net"`) |
| `email` | `""` | Jira user email for token auth |

### `[git]`

| Key | Default | Description |
|-----|---------|-------------|
| `base_branch` | `"main"` | Branch to create worktrees from |
| `remote` | `"origin"` | Git remote name |
| `repo_url` | `""` | Git repository URL (cloned during setup) |
| `repo_path` | `"/workspace"` | Path inside container |

### `[commands]`

| Key | Default | Description |
|-----|---------|-------------|
| `pre_install` | `[]` | Shell commands before install (e.g. registry auth) |
| `install` | `""` | Dependency install command (e.g. `"npm ci"`) |

### `[agent]`

| Key | Default | Description |
|-----|---------|-------------|
| `provider` | `"claude"` | `"claude"` (Claude Code CLI) or `"cursor"` (Cursor Agent CLI) |
| `cursor_cli` | `"agent"` | Cursor Agent executable name or path |
| `cursor_model` | `"Auto"` | Cursor Agent `--model` |
| `step_timeout_secs` | `1800` | Timeout per agent session (30 min) |
| `model` | `""` | Model override (e.g. `"claude-opus-4-6"`). Empty = provider default |

### `[editor]`

| Key | Default | Description |
|-----|---------|-------------|
| `ports` | `[]` | App ports to expose in the VS Code editor container |
| `dynamic_ports` | `10` | Spare ports for automatic dev-server forwarding |
| `theme` | `"vs-dark"` | VS Code theme: `"vs-dark"`, `"vs-light"`, `"hc-black"` |
| `extensions` | `[]` | VS Code extensions to install |
| `settings` | `{}` | VS Code settings as a TOML table |

### `[terminal]`

| Key | Default | Description |
|-----|---------|-------------|
| `setup_commands` | `[]` | Shell commands run once at editor container creation |

### `[web]`

| Key | Default | Description |
|-----|---------|-------------|
| `host` | `"0.0.0.0"` | Web server bind address |
| `port` | `8080` | Web server port |

### `[docker]`

| Key | Default | Description |
|-----|---------|-------------|
| `build_commands` | `[]` | Commands run during `docker compose build` |
| `compose_up_commands` | `[]` | Commands run on every `docker compose up` after auth preflight |

### `[network]`

| Key | Default | Description |
|-----|---------|-------------|
| `extra_egress_hosts` | `[]` | Additional domains allowed through the egress firewall |

---

## Browser-based editor and web terminal

Each workflow can spawn an isolated VS Code container (via [openvscode-server](https://github.com/gitpod-io/openvscode-server)) with the workflow's worktree mounted. Click **"Open editor"** on any workflow card.

Inside the editor:
- Project tools from `.mise.toml` or `[commands] install` are available
- Application ports from `[editor] ports` are pre-mapped as clickable dashboard links
- **Dynamic ports** — if `[editor] dynamic_ports > 0` (default `10`), a background scanner auto-forwards new listening ports via socat every 3 seconds
- **Web terminal** — click **"Open terminal"** for a browser-based shell (ttyd) inside the editor container

Ports are allocated from the range **9100–9200**. Close an editor with the **"Close editor"** button to stop the container and free ports.

---

## Docker-in-Docker sidecar (optional)

To run `docker` inside Maestro (e.g. nested `docker run`, Playwright containers), merge the DinD sidecar:

```bash
# In .env:
COMPOSE_FILE=docker-compose.yml:docker-compose.dind.yml
docker compose up -d
make load-worker   # load the Maestro image into DinD so worker containers start
```

Each workflow's install and agent steps then run in ephemeral Docker containers — preventing port conflicts and filesystem side-effects between concurrent workflows.

---

## Dry mode

Set `dry_mode = true` in `config.toml` to run the full pipeline without any Jira/GitHub writes. Worktrees, installs, and agent sessions still execute. Useful for testing your workflow definition before going live.

---

## Security and operations

> **Maestro runs AI agents autonomously and unattended.** The mitigations below protect your codebase and data.

### Branch protection (required)

Agents push branches and open PRs — they never commit directly to `main`. Enforce this at the Git host:

- **GitHub:** enable branch protection on `main`: require at least one human approving review, enable status checks, disable direct pushes.
- **GitLab:** use protected branches with "Maintainer" merge access and approval rules.

Without branch protection, a prompt-injection attack in a ticket description could instruct the agent to force-push without review.

### Scoped Jira tokens (required)

Use a dedicated service account or scoped API token — not personal admin credentials. Grant only **Browse Projects**, **Create Issues**, and **Assign Issues** on target projects. Using an admin token means a successful prompt-injection attack can read or modify any project on your instance.

### Scoped GitHub token (recommended)

Use a fine-grained PAT scoped to the target repository:

| Permission | Access | Used for |
|---|---|---|
| Contents | Read & write | `git push` |
| Pull requests | Read & write | `gh pr create`, PR merge polling |
| Metadata | Read | Required base permission |
| Issues | Read & write | Only if `ticketing_system = "github"` |

Set via `GH_TOKEN` in `maestro.env` — no interactive login needed.

### Other mitigations

- **Untrusted ticket text** (descriptions, linked issues) is embedded in AI prompts as `{ticket_context}`. Use Jira permissions, branch protection, and human code review. Maestro adds explicit `UNTRUSTED_JIRA` framing and optional `[jira]` byte caps; that reduces prompt-injection risk but does not eliminate it.
- **Dashboard `PUT /api/config`** only accepts `web` (login) and `general.max_concurrent_workflows` / `max_active_workflows` — **strict JSON**; anything else returns 400. Change all other settings in `config.toml` and restart.
- **Egress firewall** restricts outbound traffic to: Jira/Atlassian, GitHub, Anthropic/Claude, npm registry, and any `extra_egress_hosts` you add.

---

## Environment variables

Put secrets in `maestro.env` (mounted at `/etc/maestro/env`):

```bash
cp maestro.env.example maestro.env
```

```bash
export ANTHROPIC_BASE_URL="https://custom-proxy.example.com/claude"
export CLAUDE_CODE_OAUTH_TOKEN="your-token"
export GH_TOKEN="github_pat_..."
```

Only `export VAR=value` syntax is supported. Use `pre_install` in `config.toml` for setup commands.

---

## Container details

### Egress allowlist

Allowed by default:
- Jira/Atlassian (`api.atlassian.com` + your configured `site`)
- GitHub (`github.com`, `api.github.com`, `raw.githubusercontent.com`)
- Anthropic/Claude (`api.anthropic.com`, `api.claude.ai`, `claude.ai`, `console.anthropic.com`)
- npm registry (`registry.npmjs.org`)
- Private registries detected from `.npmrc`
- Custom hosts from `[network] extra_egress_hosts`

### Docker volumes

| Volume | Mount | Purpose |
|--------|-------|---------|
| `claude-auth` | `/home/maestro/.claude` | Claude Code auth + skills |
| `cursor-auth` | `/home/maestro/.cursor` | Cursor Agent data |
| `gh-auth` | `/home/maestro/.config/gh` | GitHub CLI auth |
| `acli-auth` | `/home/maestro/.config/acli` | Atlassian CLI auth |
| `workspace` | `/workspace` | Cloned repository |
| `npm-cache` | `/home/maestro/.npm` | npm download cache |
| `aws-config` | `/home/maestro/.aws` | AWS credentials (optional) |

### AWS CodeArtifact (if needed)

```bash
podman run --rm -v maestro_aws-config:/data -v ~/.aws:/src:ro alpine cp -r /src/. /data/
```

```toml
[commands]
pre_install = ["aws codeartifact login --tool npm --repository REPO --domain DOMAIN --domain-owner OWNER_ID"]

[network]
extra_egress_hosts = ["yourcompany-123456.d.codeartifact.region.amazonaws.com"]
```

### Project skills (`./skills`)

Add a `skills/` directory at the Maestro project root (gitignored). Skills are merged into Claude/Cursor's skills directory on every container start. For Claude in `--bare` mode, Maestro injects skill content via `--system-prompt`. For Cursor, skills are invoked natively.

### Non-root execution

The container starts as root (for iptables setup), then switches to the `maestro` user. Claude Code requires non-root execution for `--allow-dangerously-skip-permissions`.

### Logs

Per-workflow log files: `/workspace/logs/<TICKET-KEY>.log`

```bash
docker exec <container> cat /workspace/logs/PROJ-42.log
```

---

## Troubleshooting

### "Exit handler never called" during npm ci

Your npm registry is blocked by egress rules. Check the debug log:
```bash
docker exec -u maestro <container> tail -30 /home/maestro/.npm/_logs/*-debug-0.log
```
Add the registry domain to `[network] extra_egress_hosts`.

### Claude Code "api_retry error: unknown"

The Anthropic API endpoint is blocked. Verify:
```bash
docker exec -u maestro <container> curl -s -o /dev/null -w "%{http_code}" https://api.claude.ai
```
Add missing domains to `extra_egress_hosts`.

### Auth not found after rebuild

Auth is in Docker volumes. If volumes were deleted, re-run setup:
```bash
docker compose run --rm -it maestro setup
```

### Cursor `agent login`: `bad option: --use-system-ca`

Rebuild the image — you're on an older layer that used Node 20. The current image ships Node 23:
```bash
docker compose build --no-cache
```

### Cursor `agent login`: `/usr/local/bin/node: No such file or directory`

Rebuild the image — the Node binary was removed from an old layer.

### Cursor `agent login`: `Cannot find module '/usr/local/bin/index.js'`

Rebuild the image — the full cursor-agent tree must be present:
```bash
docker compose build --no-cache
```

### Project tool versions (`mise`)

The image installs [mise](https://mise.jdx.dev/) and builds tools on first run. Tool installs persist in `mise-data` and `mise-cache` volumes. Repositories can pin Node, Python, etc. via `.mise.toml` or `.tool-versions`.

### `docker compose up` stalls after "Egress rules applied"

Auth preflight is running. A hang here is usually `agent status` blocking without a TTY. Rebuild the image. For Cursor, set `CURSOR_API_KEY` in `maestro.env` to skip interactive auth checks.

### Podman on Linux with SELinux

`docker-compose.yml` sets `security_opt: [label=disable]`. If you must keep SELinux labeling, relabel the volume from the host (bind mounts with `:z`/`:Z`).

### Container name issues with Podman

```bash
podman stop -a && podman rm -f $(podman ps -aq) 2>/dev/null
podman pod rm -f $(podman pod ls -q) 2>/dev/null
```

---

## Development

### Build locally

```bash
# React dashboard (required before cargo build)
cd ui && npm install --legacy-peer-deps && npm run build && cd ..

# Rust binary (embeds ui/dist/ via rust-embed)
cargo build
cargo test
```

Or use the Makefile:
```bash
make ui-build   # React build only
make build      # React + Rust + Docker image
```

### Dashboard UI development

```bash
cd ui
npm install --legacy-peer-deps
npm run dev     # Vite dev server on :5173, proxies /api and /ws to localhost:8080
```

**Tech stack:** Vite, React 19, TypeScript, Tailwind CSS v4, React Router v7, vite-plugin-pwa, marked + DOMPurify, mermaid.

### Project structure

```
ui/                  # React + TypeScript dashboard (Vite PWA)
  src/
    api/             # API client + TypeScript types
    hooks/           # useAuth, useWebSocket, useWorkflows, usePolling
    components/      # IssueCard, modals, icons, etc.
    pages/           # Dashboard, Login, Config
crates/
  maestro-core/      # Workflow engine, Jira/GitHub/Claude integrations, config
  maestro-web/       # Axum web server, REST API, WebSocket
  maestro-cli/       # CLI entry point
docker/
  entrypoint.sh      # Container entrypoint
  egress-rules.sh    # iptables egress allowlist
workflows/           # TOML workflow definitions (*.example.toml → copy and edit)
```

---

## License

Maestro Core is source-available under the [Functional Source License 1.1 (FSL-1.1-ALv2)](LICENSE).

Self-hosting is free. If you offer Maestro as a service to others, the FSL does not permit using it to offer a competing product or hosted service. For a commercial license, see morphet.contact@gmail.com.
