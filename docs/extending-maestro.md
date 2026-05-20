# Extending Maestro

This document describes how to add tools, libraries, and customizations to
your Maestro deployment without forking the upstream image.

Maestro uses a **three-tier extension model**:

| Tier | Mechanism | When to use |
|---|---|---|
| **Baked** | This Dockerfile | Required for advertised Maestro features |
| **Provisioning** | `[provisioning].install_commands` in `config.toml` | Admin preferences — single-binary tools, version pins |
| **Custom image** | Custom Dockerfile `FROM maestro:latest` | Specialized cases — system packages, multi-path installs |

Plus a fourth mechanism for runtime-only customizations:

| Tier | Mechanism | When to use |
|---|---|---|
| **Compose override** | `docker-compose.override.yml` | Extra environment variables, extra bind mounts |

The rest of this document is the authoritative reference for each.

## The three-tier principle

> **Baked** = required for advertised features. **Provisioning** = admin preferences. **Removed** = specialized use cases.

- **Baked** tools live in the official Maestro image. The bake set is small and
  load-bearing: every binary here exists because at least one advertised
  Maestro feature needs it. Adding a tool to the bake increases image size
  for every deployment forever, so the bar is high.
- **Provisioning** tools live in the shared `maestro-tools` Docker volume.
  Admins enable them by editing `[provisioning].install_commands` in
  `config.toml`. The volume is bind-mounted READ-ONLY into every worker /
  editor / run-command Maestro spawns, and `/opt/maestro-tools/bin` is FIRST
  on `$PATH` — so anything dropped here is available everywhere AND shadows
  baked tools of the same name (the admin's pin-a-version lever).
- **Removed** tools used to be baked but were moved out because the
  cost/benefit didn't justify shipping them to every deployment. The Maestro
  authors keep an explicit list (see [Current tool inventory](#current-tool-inventory))
  documenting which tools fell into which tier and why.

## Decision guide

You want to add `<tool>`. Which tier?

| Your need | Mechanism | Example |
|---|---|---|
| Single binary CLI tool (kubectl, terraform, vendor CLI) | `[provisioning]` | `kubectl` for k8s deploys |
| Pin a baked tool to a specific version | `[provisioning]` (PATH shadowing) | `claude@2.1.140` instead of `@latest` |
| Override a baked tool's binary entirely | `[provisioning]` (PATH shadowing) | Patched build of cursor-agent |
| Add system packages (apt, libraries) | Custom Dockerfile `FROM maestro:latest` | `apt-get install kubectl awscli` |
| Add Python / Ruby / Java runtime | Custom Dockerfile or `mise` via provisioning | `mise install python@3.12` |
| Set environment variables everywhere | `docker-compose.override.yml` | `MY_API_TOKEN=...` |
| Mount an extra directory into workers | `docker-compose.override.yml` | Shared cache, host config |
| Replace the Maestro binary itself | Custom Dockerfile (rebuild) | Local development build |

## Mechanism 1 — `[provisioning].install_commands`

### What it does

At every maestro startup, the entrypoint runs the SHA-gated install pass:

1. Compute the canonical SHA-256 of the (JSON-encoded) `install_commands` list.
2. Read `/opt/maestro-tools/.provisioning-sha`.
3. If the two SHAs match → **skip** the install pass. This is the fast path
   and runs every boot when nothing has changed.
4. If they differ (or the file is missing): export `MAESTRO_TOOLS_BIN=/opt/maestro-tools/bin`,
   then iterate the list and run each command via `bash -c "$cmd"` as **root**.
   Per-command failures log a WARNING but do NOT abort. The new SHA is
   recorded ONLY on full success — partial-failure boots retry the
   failing commands on the next restart.

### Where the tools live

- Maestro container: `/opt/maestro-tools/bin` (read-write — only the
  entrypoint writes here).
- Every spawned container (workers, editors, run-commands): the same path,
  but **read-only**, via the `maestro-tools` named Docker volume.
- `$PATH`: prepended with `/opt/maestro-tools/bin`, so anything in the
  volume **wins over** baked tools of the same name.

### What goes in a command

- The string is passed to `bash -c "$cmd"`. You can use pipes, conditionals,
  command substitution, etc.
- `$MAESTRO_TOOLS_BIN` is exported to `/opt/maestro-tools/bin` — use it as
  the install target.
- Commands run as **root**. To run as the `maestro` user (e.g. `mise install`
  populating a user-scoped store), wrap with `runuser -u maestro -- …`.

### Idempotency is the admin's responsibility

Re-runs happen whenever the SHA changes — including when the admin ADDS a
new command (the existing commands re-run too). Guard each command with
an existence check so re-runs don't re-fetch unchanged binaries:

```toml
'[ -f "$MAESTRO_TOOLS_BIN/kubectl" ] || (curl -fsSLo "$MAESTRO_TOOLS_BIN/kubectl" https://dl.k8s.io/release/v1.31.0/bin/linux/amd64/kubectl && chmod +x "$MAESTRO_TOOLS_BIN/kubectl")'
```

### Worked examples

#### Install `kubectl`

```toml
[provisioning]
install_commands = [
  '[ -f "$MAESTRO_TOOLS_BIN/kubectl" ] || (curl -fsSLo "$MAESTRO_TOOLS_BIN/kubectl" https://dl.k8s.io/release/v1.31.0/bin/linux/amd64/kubectl && chmod +x "$MAESTRO_TOOLS_BIN/kubectl")',
]
```

After restart, every workflow can invoke `kubectl` directly.

#### Pin `claude` to a specific version (override the baked `@latest`)

```toml
[provisioning]
install_commands = [
  '[ -f "$MAESTRO_TOOLS_BIN/claude" ] || (npm install -g --prefix "$MAESTRO_TOOLS_BIN/.npm" @anthropic-ai/claude-code@2.1.140 && ln -sf "$MAESTRO_TOOLS_BIN/.npm/bin/claude" "$MAESTRO_TOOLS_BIN/claude")',
]
```

PATH precedence makes the pinned version win over the baked `@latest`. To
revert: remove the line, restart, and `rm /opt/maestro-tools/bin/claude`.

#### Install a private vendor CLI

```toml
install_commands = [
  '[ -f "$MAESTRO_TOOLS_BIN/mycli" ] || curl -fsSL -H "Authorization: token $MYCO_TOKEN" https://internal.example.com/cli/mycli-v2.tgz | tar -xz -C "$MAESTRO_TOOLS_BIN"',
]
```

Set `MYCO_TOKEN` in `maestro.env` (mounted into the maestro container) so
the install command sees it.

#### Install AWS CLI v2 (used to be baked; admin opts in)

```toml
install_commands = [
  '[ -f "$MAESTRO_TOOLS_BIN/aws" ] || (curl -fsSL "https://awscli.amazonaws.com/awscli-exe-linux-$(uname -m).zip" -o /tmp/aws.zip && unzip -q /tmp/aws.zip -d /tmp && /tmp/aws/install --bin-dir "$MAESTRO_TOOLS_BIN" --install-dir "$MAESTRO_TOOLS_BIN/aws-cli" --update && rm -rf /tmp/aws*)',
]
```

### Force re-install without changing config

The SHA gate normally skips install commands whose contents haven't
changed. To force a re-run (e.g. you suspect a partial install left a
broken binary):

```bash
docker exec --user root maestro-core-maestro-1 \
    rm /opt/maestro-tools/.provisioning-sha
docker compose restart maestro
```

### Wipe all customizations and start fresh

```bash
docker compose down
docker volume rm maestro-core_maestro-tools
docker compose up -d
```

The next boot recreates the volume and runs the install pass from scratch
against the current `config.toml`.

## Mechanism 2 — Custom Dockerfile (`FROM maestro:latest`)

Use this for **system packages** (apt-managed libraries, daemons),
**multi-path installs** (a tool that needs files in multiple directories),
or anything that fundamentally extends the operating system.

### Worked example

```dockerfile
# my-maestro.Dockerfile
FROM ghcr.io/morphet81/maestro:latest

# Switch to root for apt operations.
USER root

# System packages your workflows need.
RUN apt-get update && apt-get install -y --no-install-recommends \
        kubectl \
        awscli \
        postgresql-client \
    && rm -rf /var/lib/apt/lists/*

# Drop back to maestro for the runtime entrypoint.
USER maestro
```

Wire it into `docker-compose.yml`:

```yaml
services:
  maestro:
    # image: ghcr.io/morphet81/maestro:latest   # ← comment out
    build:
      context: .
      dockerfile: my-maestro.Dockerfile
```

Run: `docker compose build maestro && docker compose up -d`.

### Why use this over `[provisioning]`?

- System packages need apt + root + dependencies that touch multiple
  directories (`/etc`, `/usr/share`, `/var/lib`). The `maestro-tools`
  volume only mounts `/opt/maestro-tools/bin` — a single-directory write
  scope. Provisioning is wrong for system-wide installs.
- Apt installs are also more disk-efficient when they happen at image
  build time (Docker dedupes layers; the volume gets a fresh copy per
  deployment).
- If you need the tool available even when the `maestro-tools` volume is
  unmounted or wiped, baking it is the right answer.

## Mechanism 3 — `docker-compose.override.yml`

Docker Compose automatically merges `docker-compose.override.yml` into
the main `docker-compose.yml` when running `docker compose up`. Use this
for **runtime-only** customizations that don't need a rebuild.

### Worked example — Extra environment variables

```yaml
# docker-compose.override.yml
services:
  maestro:
    environment:
      - GH_HOST=github.example.com   # internal GHE instance
      - MY_API_TOKEN=...
```

Maestro reads `GH_HOST`, `MY_API_TOKEN`, and any other env from this file
at every container start. Worker containers inherit a subset of these via
the `PASSTHROUGH_ENV` list defined in `container.rs::ContainerRunner` — see
`AGENTS.md` for which env names propagate.

### Worked example — Extra bind mount

```yaml
services:
  maestro:
    volumes:
      - /host/shared-cache:/home/maestro/.cache/shared:ro
```

If you want the mount to propagate to spawned worker containers too,
extend `WORKER_VOLUMES` in `container.rs` and rebuild — compose overrides
ONLY affect the maestro service itself.

## Current tool inventory

This table tracks which tools live in which tier as of the current
release. When the bake set changes (rare — bake decisions are
infrastructural), update this table in lockstep with the Dockerfile.

### Baked (required for advertised features)

| Tool | Why baked |
|---|---|
| `node` (v23.11.0) | Required by Claude Code, Cursor Agent, Codex CLIs |
| `npm` | Comes with Node; required to install npm-distributed CLIs |
| `rustup` + `cargo` + `rustfmt` + `clippy` (stable) | Required for Rust-project workflows (build/test) |
| `git` | Required by every workflow (clone, commit, push) |
| `gh` (GitHub CLI) | Required for GitHub-flavored ticketing and PR operations |
| `acli` (Atlassian CLI) | Required when `ticketing_system = jira` |
| `jq` | Required by `BUNDLE_SOURCING_SH` (Claude session merge) |
| `docker` (CLI) | Required to spawn worker containers (DinD) |
| `iptables` + `iproute2` | Required by egress-rules.sh (worker network isolation) |
| `claude` (`@anthropic-ai/claude-code@latest`) | The default agent provider |
| `cursor-agent` | Required when `provider = "cursor"` |
| `codex` (`@openai/codex`) | Required when `provider = "codex"` (Phase 4) |
| `opencode` | Required when `provider = "opencode"` (Phase 4) |
| `openvscode-server` | Required for the browser editor (`open_editor`) |
| `mise` | Required for per-project tool version pinning |
| `playwright` browsers | Provisioned per-project via `npx playwright install` (cache shared) |
| Various apt libs (libglib, libnss, …) | Required by Chromium for Playwright workflows |

### Provisioning defaults (admin can disable / pin / replace)

| Tool | Why provisioning | Migration history |
|---|---|---|
| `fcli` (Figma CLI) | Single-binary, common but not universal | Migrated from bake (task #48) |
| `lokalise2` (Lokalise CLI) | Single-binary, common but not universal | Migrated from bake (task #48) |
| `figma-cli` (npm) | Single-binary, common but not universal | Migrated from bake (task #48) |

### Removed (admin handles via custom image)

| Tool | Why removed |
|---|---|
| `awscli` v2 | Only required for CodeArtifact-authenticated npm registries — a minority of deployments. Migrated to "admin opt-in via custom Dockerfile or `[provisioning]` example" (task #48) |

## Troubleshooting

### "My provisioning command isn't running on restart"

The SHA gate skips re-runs when `install_commands` is unchanged. This is
intentional — without it, every restart would re-fetch every tool. Edit
something (add a comment to the command, add a no-op step) → SHA changes
→ install pass runs.

To force a re-run without editing config:

```bash
docker exec --user root maestro-core-maestro-1 \
    rm /opt/maestro-tools/.provisioning-sha
docker compose restart maestro
```

### "I added a tool to provisioning but workers don't see it"

Check the volume mount:

```bash
docker exec maestro-core-maestro-1 ls /opt/maestro-tools/bin/
```

If the tool isn't there, check the boot log for `[maestro-provisioning]
cmd N/M: WARN` lines. Common causes:

- Network error during install (curl failed).
- `$MAESTRO_TOOLS_BIN` permissions wrong (try
  `docker exec --user root maestro-core-maestro-1 ls -la /opt/maestro-tools/`).
- The install command requires a system package not in the maestro image —
  switch to a custom Dockerfile instead.

If the tool IS in the volume but workers can't see it, check the worker's
PATH:

```bash
docker exec <worker-container-id> sh -c 'echo $PATH'
```

`/opt/maestro-tools/bin` should be first. If it isn't, the worker image
diverged from the maestro image (typically because someone overrode
`MAESTRO_REGISTRY_IMAGE`).

### "I want to roll back to maestro's pinned version of a baked tool"

Just remove the override command from `[provisioning]` and restart:

```bash
docker exec --user root maestro-core-maestro-1 \
    rm /opt/maestro-tools/bin/claude   # or whichever tool
docker compose restart maestro
```

The next worker spawn picks up the baked version via PATH precedence
(since the override file is gone).

## Forward compatibility

Phase 3 (planned) introduces an admin root terminal in the dashboard
that surfaces some of these operations as UI buttons — "Re-run
provisioning", "Wipe tools volume", "View install log". The
`[provisioning]` config block and the `maestro-tools` volume are the
underlying mechanism that won't change. Future Maestro versions will
remain backward-compatible with `[provisioning]` configurations written
today.

## See also

- `README.md` — Project overview and quickstart (includes the "Extending Maestro" quickstart section).
- `AGENTS.md` — "Tool layout and extensibility" section with rules for AI agents modifying the codebase.
- `config.toml.example` — Default `[provisioning]` block with inline comments.
- `Dockerfile` — Authoritative bake list.
