<div align="center">

# Takuto Core

**AI coding agents that run isolated — and under your control.**

Turn Jira and GitHub tickets into pull requests: fully autonomous, or steered ticket by
ticket from a live dashboard. Every agent runs in its own container, behind an egress
firewall, on hardware you own.

[Documentation](https://takuto-doc.alexandre-obellianne.workers.dev) ·
[Quick start](https://takuto-doc.alexandre-obellianne.workers.dev/docs/quick-start/) ·
[Security](https://takuto-doc.alexandre-obellianne.workers.dev/security/) ·
[CLI](https://github.com/takuto-team/takuto-cli)

![License: FSL-1.1-ALv2](docs/badges/license-agpl.svg)
![Status: beta](docs/badges/status-beta.svg)
![Self-hosted](docs/badges/self-hosted.svg)

</div>

<!-- TODO(launch): replace with a 60–90s demo GIF — a ticket going in, the live dashboard
     working, a PR coming out. This is the single most persuasive asset; record it before
     announcing. Suggested path: docs/demo.gif -->
<!-- ![Takuto turning a ticket into a pull request](docs/demo.gif) -->

**What it is.** Takuto Core is a self-hosted AI coding pipeline. Point it at Jira or GitHub
Issues and let it run the whole loop — branch → install → implement → lint/tests → PR — then
move to the next ticket; or stay in the driver's seat and trigger each phase yourself. It's
**bring-your-own-agent** (Claude Code, Cursor Agent, Codex, or OpenCode for self-hosted
models), and there is **no telemetry** — your code and tickets go only to the provider you
configure.

The rest of this README goes from the high-level picture down to full operational detail.
For the polished guides, see the **[documentation site](https://takuto-doc.alexandre-obellianne.workers.dev)**.

---

## Table of contents

- [What you can achieve](#what-you-can-achieve)
- [Why Takuto?](#why-takuto)
- [You will need](#you-will-need)
- [Cost expectations](#cost-expectations)
- [Quick start — Individual developer (local)](#quick-start--individual-developer-local)
- [Quick start — Teams (server deployment)](#quick-start--teams-server-deployment)
- [How a workflow runs](#how-a-workflow-runs)
- [Multi-user model](#multi-user-model)
- [Privacy & telemetry](#privacy--telemetry)
- [Configuration reference](#configuration-reference)
- [Extending Takuto — adding tools](#extending-takuto--adding-tools)
- [Browser-based editor and web terminal](#browser-based-editor-and-web-terminal)
- [Docker-in-Docker sidecar (optional)](#docker-in-docker-sidecar-optional)
- [Dry mode](#dry-mode)
- [Security and operations](#security-and-operations)
- [Environment variables](#environment-variables)
- [Container details](#container-details)
- [Troubleshooting](#troubleshooting)
- [Development](#development)
- [Going deeper](#going-deeper)
- [License](#license)

---

## What you can achieve

- **Fully automated mode** — connect Jira or GitHub Issues and Takuto polls automatically: it picks up "To Do" tickets, assigns them, runs the full AI pipeline (worktree → install → implement → lint/tests → PR), and moves on to the next one.
- **Manual mode, your pace** — add any ticket or task to the dashboard yourself, refine its description with AI assistance before the agent ever sees it, then trigger each workflow phase when you're ready. No polling, no surprises.
- **Mix both** — auto-pick routine tasks while manually curating the tricky ones. You control which tickets get the autopilot treatment and which ones you steer yourself.
- **Run multiple tickets in parallel** — configure how many workflows run concurrently; each gets its own git worktree and isolated environment.
- **Monitor everything in real time** — a live web dashboard streams terminal output per workflow, shows progress, and lets you pause, resume, retry, or inspect any run.
- **Jump into any workflow** — open a browser-based VS Code editor and web terminal, pre-configured with your project tools, pointed at the exact worktree the agent is working on.
- **Define your own pipeline steps** — TOML workflow definitions let you chain phases: implement → address PR comments → merge base branch → deploy. Steps depend on each other; trigger them from the dashboard.
- **Work without a ticketing system** — paste any description via the dashboard and Takuto treats it as a workflow. No Jira account required.

---

## What makes it different

- **Isolated by container.** Every workflow runs in its own container and git worktree,
  behind a default-deny egress firewall — the prompt-injection blast radius is one container,
  not your machine or your network.
- **Define the architecture, then let it run.** You set the approach — the ticket/spec and the
  workflow steps — and the agent does the legwork autonomously. Step in only at the end to
  fine-tune through the in-browser VS Code editor or web terminal (pointed at the exact
  worktree) if the result needs a human touch.
- **AI-assisted ticket prep.** Sharpen a ticket with AI help *before* the agent ever sees it,
  so it works from a clear, complete spec instead of a vague one — better input, better PR.
- **Run your app from a clean container.** Custom run commands launch your dev server / app
  inside the isolated container on the server (with port forwarding), so you can preview and
  verify the agent's work in a fresh environment — no "works on my machine".

## Why Takuto?

| | IDE assistant (Copilot, Cursor inline) | Takuto |
|---|---|---|
| **Where it runs** | Inside your editor, on your machine | Inside Docker, on any machine or server |
| **Supervision required** | Yes — you approve each step | Optional — fully autonomous or manual-trigger, your choice |
| **Ticketing integration** | None | Jira, GitHub Issues, or standalone |
| **Pipeline definition** | Single prompt | Multi-step TOML: implement, review, test, PR, deploy |
| **Concurrent work** | One task at a time | Multiple tickets in parallel |
| **Security boundary** | Full internet access from agent | Egress firewall — only approved hosts reachable |
| **Team deployment** | Per-developer only | Self-host on a server; shared dashboard |
| **Persistence** | Session ends when you close your editor | Survives container restarts; paused workflows resume |

---

## You will need

Before you start, gather these:

- **Docker** (or Podman) with `docker compose` — Docker 24+ / Podman 4+ recommended.
- **RAM:** ≥ 8 GiB for a single workflow; ≥ 12 GiB on macOS with Podman because the Podman VM needs its own share. Tune `[general] max_concurrent_workflows` to your machine.
- **Disk:** ≥ 30 GiB free. Worktrees, npm/cargo caches, mise toolchains, and (if enabled) the DinD storage layer all live in Docker volumes.
- **GitHub access:** either a fine-grained personal access token (PAT) or a configured GitHub App. See [Scoped GitHub token](#scoped-github-token-recommended).
- **Atlassian CLI (optional):** required only when `[general] ticketing_system = "jira"`. Installed automatically inside the container; you authenticate from the dashboard.
- **An AI provider account** — bring your own agent:
  - **Claude Code** — Anthropic API key, Pro/Max OAuth, or a corporate proxy (`ANTHROPIC_BASE_URL`).
  - **Cursor Agent** — `CURSOR_API_KEY` or interactive `agent login`.
  - **Codex** — an OpenAI API key (`OPENAI_API_KEY`).
  - **OpenCode** — a self-hosted model server (LM Studio, Ollama, vLLM…) via its `base_url`.

A Linux host is recommended for server deployments; macOS works for local use but Podman's VM eats memory you'd rather give to the agent.

---

## Cost expectations

Takuto doesn't bill you — your AI provider does. The agent runs Claude Code or Cursor Agent against your account, and each ticket consumes tokens proportional to the size of the codebase, the prompt context, and the number of steps in your workflow definition.

What that costs depends entirely on the provider and model you choose — check their current
rates, and watch your first few runs to get a feel for your own workload. Cost scales with
codebase size, prompt context, and the number of steps in your workflow. A **self-hosted
model** via OpenCode removes per-token billing entirely (you pay only for the hardware).

Cost-saving levers:

- Use **command steps** (`commands = ["..."]`) instead of agent prompts for deterministic work like linting and testing — they don't consume AI tokens.
- Cap context with `[jira] ticket_context_max_description_bytes` and `linked_issue_description_max_bytes`.
- Run a **dry mode** rehearsal of your workflow definition (`[general] dry_mode = true`) before pointing it at a real backlog.
- Claude Code Pro/Max OAuth (via `claude login`) uses your subscription rather than per-token API billing.

---

## Quick start — Individual developer (local)

Run Takuto on your laptop. Takes about 10 minutes.

### 1. Configure

```bash
cp config.toml.example config.toml
cp takuto.env.example takuto.env
```

Edit `config.toml`:
- Set `[general] ticketing_system` to `"jira"`, `"github"`, or `"none"`
- For Jira: fill in `[jira] site`, `project_keys`, `email`
- For GitHub Issues: the repo is detected from the cloned repository's git remote
- Clone your repository via the dashboard "Setup a New Project" button, or manually into `/workspace`

### 2. Build

```bash
docker compose build
```

### 3. Start

```bash
docker compose up
```

Dashboard at **http://localhost:8080**. On first load it prompts you to create the initial **admin account** — no credentials live in `config.toml`.

**Podman on macOS:** increase the default machine resources first:
```bash
podman machine stop && podman machine set --memory 12288 --cpus 4 && podman machine start
```

### 4. Add your credentials in the UI

Log in, then open **Configuration → My Credentials** and paste your **provider API key** (Claude / Cursor / OpenAI) and a **GitHub token (PAT)**. This is the recommended path — the app starts and is fully reachable **without** any CLI auth step.

> **Optional — OAuth login (not recommended).** If you prefer interactive OAuth for Claude/Cursor/GitHub instead of API keys, run `docker compose run --rm -it takuto setup` first (it walks through GitHub CLI → Atlassian CLI (optional) → Claude Code / Cursor Agent login). The app works fine without it; API keys entered in the UI are simpler.

If you use Jira or GitHub Issues, Takuto starts polling automatically. Otherwise, click **+** to paste a description and kick off a workflow manually.

---

## Quick start — Teams (server deployment)

Self-host Takuto on a Linux server. Your team accesses the dashboard through a browser; the agent runs in the background.

### 1. Clone and configure on the server

```bash
git clone <takuto-repo-url> && cd takuto-core
cp config.toml.example config.toml
cp takuto.env.example takuto.env
```

Key settings for server deployments:

```toml
[general]
ticketing_system = "jira"          # or "github"
max_concurrent_workflows = 3       # tune to your server's CPU/RAM

[web]
host = "0.0.0.0"
port = 8080
```

> Takuto uses a multi-user database for authentication — on first boot the
> dashboard prompts you to create the initial admin account. See
> [Multi-user model](#multi-user-model).
>
> **Upgrading?** The legacy single-user keys `[web] dashboard_username` /
> `dashboard_password` have been **removed**. If your old `config.toml` still
> contains them they are silently ignored — create your admin account on the
> first-boot setup page (or `POST /api/auth/register`) instead.

Put secrets in `takuto.env` (never in `config.toml`):
```bash
export ANTHROPIC_API_KEY="sk-ant-..."
export GH_TOKEN="github_pat_..."
```

### 2. Build

```bash
docker compose build
```

> **Optional — OAuth login (not recommended).** `docker compose run --rm -it takuto setup` authenticates Claude/Cursor/GitHub via interactive OAuth. You don't need it: start the server (next step), create the admin account on first load, and have each user paste their API keys under **Configuration → My Credentials**. The app is reachable without this step.

### 3. Start as a service

```bash
docker compose up -d
```

### 4. Expose with a reverse proxy (recommended)

Point nginx or Caddy at port 8080 and terminate TLS there. Takuto listens on plain HTTP; put HTTPS termination in front.

Example Caddy snippet:
```
takuto.yourcompany.com {
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
3. **Worktree init** — runs your per-workspace init commands (e.g. `npm ci`) in the worktree, configured in **Configuration → Worktree Settings**
4. **Agent steps** — one or more AI sessions using your TOML workflow definition; each step has a prompt with ticket context injected
5. **Done** — dashboard shows PR URL (from `.takuto/outcome.toml` or `TAKUTO_PR_URL:` in agent output)

On **stop**: active sessions killed, ticket reverted to "To Do".

### Dynamic workflow definitions

Drop `*.toml` files in the `workflows/` directory to define your pipeline. Start from the examples:

```bash
cp workflows/implement_ticket.example.toml workflows/implement_ticket.toml
```

Each definition has `[[steps]]` with a `prompt` (ticket context auto-injected) or `commands`. Chain definitions with `depends_on` — a "merge base" step only becomes available after "implement ticket" completes.

---

## Multi-user model

Takuto is multi-user, single-tenant. Every user has their own dashboard view; all users on one instance share the same Jira, GitHub, and AI credentials.

**On first boot,** when the database has zero users, the dashboard shows a **first-user setup** page. The account you create there becomes the initial **admin**.

**Roles:**

- **admin** — can create, edit, suspend, and delete users; can mutate shared state (`PUT /api/config`, polling pause/resume, workspace switch, repo clone).
- **user** — sees and acts only on workflows they created. No cross-user visibility, even for admins.

**Sign-in flow:**

- Username + password, argon2-hashed in `takuto.db`.
- Idle session TTL: 24 hours. Absolute TTL: 30 days.
- After 5 failed login or recovery attempts in 10 minutes the account is temporarily locked; an admin clears it via `POST /api/users/{id}/unlock`.
- Per-IP rate limit on `/api/auth/login` and `/api/auth/recover`: 10 / minute.
- One-time **recovery codes** are issued at account creation and can reset a forgotten password without admin intervention.
- By default a new login kicks other sessions for the same user (`[web] kick_other_sessions_on_login = true`). Flip to `false` if you want concurrent desktop + mobile sessions.

**Workflow isolation:** each `Workflow` carries a `user_id`. Users see only their own workflows on the dashboard and via `GET /api/workflows`. Workflows created automatically by the poller are owned by the user resolved via `[general] poller_owner_username` (defaults to the lexicographically-first admin).

**Workspace switching is global.** When an admin switches workspaces (`POST /api/workspaces/switch`), running workflows from the previous workspace keep executing — but the dashboard scopes its grid to the newly-active workspace. Cross-workspace impact is documented in [AGENTS.md](AGENTS.md) under "Workflow model".

User management UI lives at **Configuration → Users** (admin-only). For the full data model and migration path from single-user deployments, see the **Multi-user database** section in [AGENTS.md](AGENTS.md).

---

## Privacy & telemetry

**Takuto does not collect or transmit any telemetry.** There is no usage analytics, no crash reporter, no phone-home — verify with `git grep telemetry` if you want to.

All outbound traffic is to services *you* configure:

- Your Jira/Atlassian site (when `ticketing_system = "jira"`).
- GitHub (`github.com`, `api.github.com`, `raw.githubusercontent.com`).
- Your AI provider — Anthropic (`api.anthropic.com`, `api.claude.ai`, `claude.ai`, `console.anthropic.com`) and/or Cursor.
- npm registry (`registry.npmjs.org`) and any private registries detected in `.npmrc`.
- Any host you add to `[network] extra_egress_hosts`.

The egress firewall (iptables, set up at container start) blocks everything else by default. Ticket content, source code, and agent prompts are sent only to the AI provider you configured — Takuto itself never sees them.

---

## Configuration reference

The canonical, per-key reference for `config.toml` lives in **[docs/configuration.md](docs/configuration.md)** — every key, default, type, and description, with 🔒 callouts on security-sensitive options. Start there. The annotated `config.toml.example` at the repo root is the source of truth: if it diverges from the doc, the example wins.

A few keys you'll touch first:

| Key | Default | What it does |
|---|---|---|
| `[general] ticketing_system` | `"none"` | `"jira"`, `"github"`, or `"none"` — drives the poller and the **+** picker. |
| `[general] max_concurrent_workflows` | `1` | Parallel install/agent sessions. Tune to your host. |
| `[git] base_branch` | `"main"` | Branch worktrees are created from. |
| `[agent] provider` | `"claude"` | `"claude"`, `"cursor"`, `"codex"`, or `"opencode"`. Per-provider model/endpoint lives in `[agent.providers.<name>]` — edit from Configuration → AI Settings. |
| `[web] host` / `port` | `"0.0.0.0"` / `8080` | Dashboard bind address. |
| `[network] extra_egress_hosts` 🔒 | `[]` | Extra domains permitted through the egress firewall. |

**Heads up:** `[commands]` and `[[run_commands]]` in `config.toml` are **ignored at load** (a startup warning is logged). Worktree init commands and dashboard run/stop buttons are per-user and live in the database — edit them via **Configuration → Worktree Settings** in the dashboard. See [docs/configuration.md § Ignored sections](docs/configuration.md#ignored-sections).

---

## Extending Takuto — adding tools

You'll often need tools that aren't baked into the official image: `kubectl`, `terraform`, an internal vendor CLI, a pinned version of `claude`. Takuto has three extension paths; pick the one that matches your need.

| Your need | Mechanism | Where to write it |
|---|---|---|
| Add a single binary CLI (kubectl, terraform, internal CLI) | **Provisioning** | `[provisioning].install_commands` in `config.toml` |
| Pin a baked tool to a specific version | **Provisioning** (PATH shadowing) | `[provisioning].install_commands` |
| Add system packages (apt) or libraries | **Custom Dockerfile** | New Dockerfile `FROM ghcr.io/takuto-team/takuto-core:latest` |
| Add an environment variable everywhere | **Compose override** | `docker-compose.override.yml` |
| Mount an extra host directory | **Compose override** | `docker-compose.override.yml` |

### Quickstart — Provisioning (the most common case)

Edit `config.toml`:

```toml
[provisioning]
install_commands = [
  '[ -f "$TAKUTO_TOOLS_BIN/kubectl" ] || (curl -fsSLo "$TAKUTO_TOOLS_BIN/kubectl" https://dl.k8s.io/release/v1.31.0/bin/linux/amd64/kubectl && chmod +x "$TAKUTO_TOOLS_BIN/kubectl")',
]
```

Restart takuto — `kubectl` is now available to every workflow, the in-browser editor, every run-command. The install is SHA-gated: rebooting with the same `install_commands` list is a no-op (fast path); editing the list re-runs the install pass.

### Quickstart — Pin claude to a specific version

```toml
[provisioning]
install_commands = [
  '[ -f "$TAKUTO_TOOLS_BIN/claude" ] || (npm install -g --prefix "$TAKUTO_TOOLS_BIN/.npm" @anthropic-ai/claude-code@2.1.140 && ln -sf "$TAKUTO_TOOLS_BIN/.npm/bin/claude" "$TAKUTO_TOOLS_BIN/claude")',
]
```

PATH precedence makes the pinned version win over the baked `@latest`. The `takuto-tools` volume is bind-mounted into every Takuto-spawned container with `$PATH` prepended so the override propagates everywhere.

### Quickstart — System packages via custom image

```dockerfile
# my-takuto.Dockerfile
FROM ghcr.io/takuto-team/takuto-core:latest
USER root
RUN apt-get update && apt-get install -y --no-install-recommends \
        awscli postgresql-client \
    && rm -rf /var/lib/apt/lists/*
USER takuto
```

Wire it into `docker-compose.yml`:

```yaml
services:
  takuto:
    build:
      context: .
      dockerfile: my-takuto.Dockerfile
```

### Full reference

See **[docs/extending-takuto.md](docs/extending-takuto.md)** for:
- The three-tier model (Baked / Provisioning / Custom Image) with rationale.
- The full `[provisioning]` reference: `$TAKUTO_TOOLS_BIN`, idempotency rules, SHA gate behavior, force-reinstall escape hatches.
- The current tool inventory: which tools are baked, which are provisioning defaults, which were removed.
- Troubleshooting common provisioning issues.

---

## Browser-based editor and web terminal

Each workflow can spawn an isolated VS Code container (via [openvscode-server](https://github.com/gitpod-io/openvscode-server)) with the workflow's worktree mounted. Click **"Open editor"** on any workflow card.

Inside the editor:
- Project tools from `.mise.toml` or your **Worktree Settings** init commands are available
- Application ports from `[editor] ports` are pre-mapped as clickable dashboard links
- **Dynamic ports** — if `[editor] dynamic_ports > 0` (default `10`), a background scanner auto-forwards new listening ports via socat every 3 seconds
- **Web terminal** — click **"Open terminal"** for a browser-based shell (ttyd) inside the editor container

Ports are allocated from the range **9100–9200**. Close an editor with the **"Close editor"** button to stop the container and free ports.

---

## Docker-in-Docker sidecar (optional)

To run `docker` inside Takuto (e.g. nested `docker run`, Playwright containers), merge the DinD sidecar:

```bash
# In .env:
COMPOSE_FILE=docker-compose.yml:docker-compose.dind.yml
docker compose up -d
make load-worker   # load the Takuto image into DinD so worker containers start
```

Each workflow's install and agent steps then run in ephemeral Docker containers — preventing port conflicts and filesystem side-effects between concurrent workflows.

---

## Self-hosted model server bridge (optional)

**You only need this when running a local model server on the host machine.** If you use the OpenCode provider against a model server (LM Studio, Ollama, vLLM, …) running **on your host Mac**, Docker Desktop 4.34+'s gVisor network stack blocks the worker containers from reaching `host.docker.internal`. Add the bridge so the workers can reach the host:

```bash
make start BACKEND=postgres LM_BRIDGE=1
# Ollama (port 11434) or any non-default port:
LM_HOST_PORT=11434 make start BACKEND=postgres LM_BRIDGE=1
```

This starts a small `socat` sidecar (`takuto-lm-bridge`) at a fixed address. Then in **Configuration → AI Settings → OpenCode → Base URL** set:

```
http://172.20.0.250:1234/v1
```

**You do NOT need `LM_BRIDGE` if:**

- you use a **cloud provider** (Claude, Codex, or any reachable API URL), or
- your model server runs **as a container inside the Takuto compose network** — reach it directly by service name (e.g. `http://lm-studio:1234/v1`).

See [`docs/troubleshooting-self-hosted-models.md`](docs/troubleshooting-self-hosted-models.md) for the full diagnosis and a smoke test.

---

## Dry mode

Set `dry_mode = true` in `config.toml` to run the full pipeline without any Jira/GitHub writes. Worktrees, installs, and agent sessions still execute. Useful for testing your workflow definition before going live.

---

## Security and operations

> **Takuto runs AI agents autonomously and unattended.** The mitigations below protect your codebase and data.

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

Set via `GH_TOKEN` in `takuto.env` — no interactive login needed.

### Other mitigations

- **Untrusted ticket text** (descriptions, linked issues) is embedded in AI prompts as `{ticket_context}`. Use Jira permissions, branch protection, and human code review. Takuto adds explicit `UNTRUSTED_JIRA` framing and optional `[jira]` byte caps; that reduces prompt-injection risk but does not eliminate it.
- **Dashboard `PUT /api/config`** only accepts `web` (login) and `general.max_concurrent_workflows` / `max_active_workflows` — **strict JSON**; anything else returns 400. Change all other settings in `config.toml` and restart.
- **Egress firewall** restricts outbound traffic to: Jira/Atlassian, GitHub, Anthropic/Claude, npm registry, and any `extra_egress_hosts` you add.

---

## Environment variables

Put secrets in `takuto.env` (mounted at `/etc/takuto/env`):

```bash
cp takuto.env.example takuto.env
```

```bash
export ANTHROPIC_BASE_URL="https://custom-proxy.example.com/claude"
export CLAUDE_CODE_OAUTH_TOKEN="your-token"
export GH_TOKEN="github_pat_..."
```

Only `export VAR=value` syntax is supported. For per-workspace setup commands, use **Configuration → Worktree Settings** in the dashboard.

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
| `claude-auth` | `/home/takuto/.claude` | Claude Code auth + skills |
| `cursor-auth` | `/home/takuto/.cursor` | Cursor Agent data |
| `gh-auth` | `/home/takuto/.config/gh` | GitHub CLI auth |
| `acli-auth` | `/home/takuto/.config/acli` | Atlassian CLI auth |
| `workspace` | `/workspace` | Cloned repository |
| `npm-cache` | `/home/takuto/.npm` | npm download cache |
| `aws-config` | `/home/takuto/.aws` | AWS credentials (optional) |

### AWS CodeArtifact (if needed)

```bash
podman run --rm -v takuto_aws-config:/data -v ~/.aws:/src:ro alpine cp -r /src/. /data/
```

Add the `codeartifact login` step to your per-workspace init commands in
**Configuration → Worktree Settings**:

```
aws codeartifact login --tool npm --repository REPO --domain DOMAIN --domain-owner OWNER_ID
```

and allow the registry host through the egress firewall in `config.toml`:

```toml
[network]
extra_egress_hosts = ["yourcompany-123456.d.codeartifact.region.amazonaws.com"]
```

### Project skills (`./skills`)

Add a `skills/` directory at the Takuto project root (gitignored). Skills are merged into Claude/Cursor's skills directory on every container start. For Claude in `--bare` mode, Takuto injects skill content via `--system-prompt`. For Cursor, skills are invoked natively.

### Non-root execution

The container starts as root (for iptables setup), then switches to the `takuto` user. Claude Code requires non-root execution for `--allow-dangerously-skip-permissions`.

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
docker exec -u takuto <container> tail -30 /home/takuto/.npm/_logs/*-debug-0.log
```
Add the registry domain to `[network] extra_egress_hosts`.

### Claude Code "api_retry error: unknown"

The Anthropic API endpoint is blocked. Verify:
```bash
docker exec -u takuto <container> curl -s -o /dev/null -w "%{http_code}" https://api.claude.ai
```
Add missing domains to `extra_egress_hosts`.

### Auth not found after rebuild

Auth is in Docker volumes. If volumes were deleted, re-run setup:
```bash
docker compose run --rm -it takuto setup
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

Auth preflight is running. A hang here is usually `agent status` blocking without a TTY. Rebuild the image. For Cursor, set `CURSOR_API_KEY` in `takuto.env` to skip interactive auth checks.

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
  takuto-core/      # Workflow engine, Jira/GitHub/Claude integrations, config
  takuto-web/       # Axum web server, REST API, WebSocket
  takuto-cli/       # CLI entry point
docker/
  entrypoint.sh      # Container entrypoint
  egress-rules.sh    # iptables egress allowlist
workflows/           # TOML workflow definitions (*.example.toml → copy and edit)
```

---

## Going deeper

- [AGENTS.md](AGENTS.md) — canonical map of architecture, runtime paths, REST/WebSocket contracts. Read this first if you're contributing or hacking on Takuto.
- [ARCHITECTURE.md](ARCHITECTURE.md) — crate-by-crate breakdown and diagrams.
- [CODING_STANDARDS.md](CODING_STANDARDS.md) — SOLID, Rust, React/TypeScript, and security rules every contributor follows.
- [docs/workflow.md](docs/workflow.md) — Mermaid diagrams for the ticket lifecycle.
- [docs/configuration.md](docs/configuration.md) — full configuration reference.
- [SECURITY.md](SECURITY.md) — supported versions, reporting a vulnerability, trust model.
- [CONTRIBUTING.md](CONTRIBUTING.md) — dev setup, DCO sign-off, license-header rules.

---

## License

Takuto Core is source-available under the [Functional Source License 1.1 (FSL-1.1-ALv2)](LICENSE).

Self-hosting is free. If you offer Takuto as a service to others, the FSL does not permit using it to offer a competing product or hosted service. For a commercial license, see morphet.contact@gmail.com.
