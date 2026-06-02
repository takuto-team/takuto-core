# Configuration reference

Canonical reference for every key in `config.toml`. Source of truth is
[`config.toml.example`](../config.toml.example) at the repository root — when
this page and that file disagree, the example wins.

Settings are reloaded on file change (5-second poller). A few keys are
**startup-only** and require a restart (noted inline). Secrets belong in
`maestro.env`, never in `config.toml`.

## Conventions

- 🔒 — security-sensitive. Review before changing in a shared deployment.
- *(deprecated)* — supported for backward compatibility; will be removed in a
  future release.
- **Default** column shows the value Maestro uses when the key is absent.
- Commented-out keys in `config.toml.example` show their default in the comment;
  this page lists the same defaults explicitly.

## Table of contents

- [`[general]`](#general)
- [`[jira]`](#jira)
- [`[git]`](#git)
- [`[github]`](#github)
- [`[web]`](#web)
- [`[agent]`](#agent)
- [`[docker]`](#docker)
- [`[editor]`](#editor)
- [`[terminal]`](#terminal)
- [`[network]`](#network)
- [`[dev]`](#dev)
- [`[provisioning]`](#provisioning)
- [Ignored sections](#ignored-sections)

---

## `[general]`

Process-wide behaviour: ticketing mode, polling, concurrency, logging.

| Key | Default | Type | Description |
|---|---|---|---|
| `ticketing_system` | `"none"` | string | `"jira"`, `"github"`, or `"none"`. Drives the poller and the dashboard **+** button. |
| `dry_mode` 🔒 | `false` | bool | Skip real Jira/GitHub side effects (no assigns, transitions, or pushes). Local work — worktrees, installs, agent sessions — still runs. Useful for staging a workflow definition before going live. |
| `auto_polling` | `true` | bool | When `false`, polling starts paused at boot (same as clicking **Pause polling** on the dashboard). Use **Resume polling** to pick up new tickets. |
| `poll_interval_secs` | `60` | int | Seconds between Jira/GitHub polls. |
| `pr_merge_poll_interval_secs` | `60` | int | Seconds between polls that check whether a workflow's PR has been merged on GitHub. `0` disables. |
| `max_concurrent_workflows` | `1` | int | Semaphore size for parallel mise/install/agent sessions. Tune to your server's CPU and RAM. |
| `max_active_workflows` | `0` | int | Max workflows visible on the dashboard (every row including **Done**, **Paused**, **Stopped**, **Error**). The poller will not start new tickets while at this limit. `0` mirrors `max_concurrent_workflows`. |
| `max_concurrent_manual_workflows` | `0` | int | Cap on dashboard **+** manual starts that are not **Done**/**Stopped**/**Error**. `0` = no limit. The **+** tile is disabled when full. |
| `generate_report` | `false` | bool | When `true`, each agent step appends findings to `lore/reports/<item-key>_report.md` and a final consolidation step produces a polished summary. Adds tokens — leave `false` unless you want the **Report** button populated. |
| `poller_owner_username` | *(unset)* | string | Username of the user who owns workflows created automatically by the poller. When unset, the lexicographically-first non-suspended admin is used. When set but missing/suspended, an admin fallback applies with a warning. When neither resolves, the poller skips entirely. |
| `migrate_orphan_workflows` | `false` | bool | When `true`, restored workflows with `user_id = None` (pre-multi-user orphans) are reassigned to the resolved poller owner at startup. |
| `migrate_orphan_repo_associations` | `true` | bool | When `true`, startup reconciliation creates a `user_repositories` association for every restored snapshot workflow that has a `user_id` and whose `workspace_name` matches a registered repository. Set to `false` to require users to re-add the repository explicitly. |
| `log_level` | `"info"` | string | `trace`, `debug`, `info`, `warn`, `error`. Applied via `tracing_subscriber` `EnvFilter`. |
| `worker_image` | `""` | string | Docker image for isolated workflow containers when DinD is enabled. Empty = auto-detect from the running Maestro image. |
| `workflow_definitions_dir` | `"workflows"` | string | Path scanned for `*.toml` workflow definitions, resolved relative to the config file's directory. |

---

## `[jira]`

Only used when `[general] ticketing_system = "jira"`.

| Key | Default | Type | Description |
|---|---|---|---|
| `project_keys` | `[]` | array<string> | Jira project keys to poll (e.g. `["PROJ", "ENG"]`). The dashboard **+** picker is hidden when this is empty. |
| `item_types` | `["Task", "Bug"]` | array<string> | Ticket types the poller picks up. The manual **+** picker ignores this and shows all non-Epic issues. |
| `done_status` | `"Done"` | string | Transition name fired by **Mark as Done**. Must match a real transition in your Jira workflow. |
| `jql_filter` | `""` | string | Extra JQL `AND`-merged into the dashboard **+** ticket picker (e.g. a board filter fragment). Epics are always excluded. |
| `site` | `""` | string | Jira host or full `https://` URL (e.g. `"yourcompany.atlassian.net"`). Empty = `jira.atlassian.net`. Drives token auth, egress allowlist, and PR-body Jira link. |
| `email` | `""` | string | Jira user email for `acli` token auth. |
| `linked_items_in_prompt` | `"full"` | string | `"full"` \| `"summary_only"` \| `"omit"` — how linked-issue text appears in `{ticket_context}`. |
| `ticket_context_max_description_bytes` | `100000` | int | Cap on the main ticket description in `{ticket_context}`. `0` = unlimited. |
| `linked_issue_description_max_bytes` | `32000` | int | Cap per linked-issue description when `linked_items_in_prompt = "full"`. `0` = unlimited. |
| `acli_allowed_extra_prefixes` | `[]` | array<string> | Advanced: extra allowed `acli` argv prefixes (tokens per line). |

---

## `[git]`

| Key | Default | Type | Description |
|---|---|---|---|
| `base_branch` | `"main"` | string | Branch each worktree is created from. |
| `remote` | `"origin"` | string | Git remote used for fetch, base ref, and push. |
| `repo_path` | `"/workspace"` | string | Path to the cloned repo inside the container. The workspace name is derived from this path's last component. |

---

## `[github]`

Optional GitHub App authentication. When configured, commits and PRs are attributed
to `maestro-bot[bot]` instead of the personal `gh` user.

All three fields (`app_id`, `app_installation_id`, and one private key source)
must be set together. Errors are non-fatal at startup — Maestro falls back to
personal `gh` auth and logs a warning.

Required App permissions: contents (write), pull_requests (write), metadata (read).

| Key | Default | Type | Description |
|---|---|---|---|
| `app_id` | *(unset)* | int | GitHub App ID. |
| `app_installation_id` | *(unset)* | int | App installation ID for your org/repo. |
| `app_private_key` 🔒 | *(unset)* | string | PEM-encoded RSA private key, inline. Mutually exclusive with `app_private_key_path`. Prefer keeping this in `maestro.env` if you can — see [Environment variables](#environment-variables) in the README. |
| `app_private_key_path` 🔒 | *(unset)* | string | Path to a PEM file with the App's private key. |
| `gh_allowed_extra_prefixes` | `[]` | array<string> | Extra `gh` argv prefixes allowed beyond the built-in set (`api`, `pr create`, `pr edit`, `auth login`, `auth setup-git`, `auth status`). Each entry is a whitespace-separated token line. Set to `["*"]` to disable the gh allowlist entirely. |

---

## `[web]`

Dashboard server.

| Key | Default | Type | Description |
|---|---|---|---|
| `host` | `"0.0.0.0"` | string | Bind address. |
| `port` | `8080` | int | Bind port. |
| `dashboard_username` 🔒 *(deprecated)* | `""` | string | Legacy single-user auth. Multi-user DB auth is the supported path (see [Multi-user model](../README.md#multi-user-model)). Leave empty in new deployments. |
| `dashboard_password` 🔒 *(deprecated)* | `""` | string | Legacy single-user password. `GET /api/config` never returns this value. Leave empty in new deployments. |
| `cors_origins` 🔒 | `[]` | array<string> | Allowed CORS origins. Empty = auto-compute from `host` and `port` (e.g. `http://localhost:8080`). Set explicitly when behind a reverse proxy or TLS terminator (e.g. `["https://maestro.example.com"]`). **Startup-only** — requires restart. |
| `cookie_secure` 🔒 | *(auto-detect)* | bool | Controls the `Secure` flag on the session cookie. Unset: auto-detect (set when any `cors_origins` entry is `https://…` OR when the inbound request carries `X-Forwarded-Proto: https`). Force `true` when terminating TLS in front of Maestro without `X-Forwarded-Proto`; use `false` for local plain-HTTP testing only. |
| `kick_other_sessions_on_login` | `true` | bool | When `true`, a successful login deletes all prior sessions for the same user (single-session enforcement). Set `false` for long-lived multi-client sessions (desktop + mobile). |

**Session policies (not configurable):**

- Idle session TTL: 24 hours.
- Absolute session TTL: 30 days.
- Account lockout: after 5 failed login or recovery attempts in 10 minutes (admin clears via `POST /api/users/{id}/unlock`).
- Per-IP rate limit on `/api/auth/login` and `/api/auth/recover`: 10 / minute.

---

## `[agent]`

AI provider for `prompt`-bearing workflow steps.

| Key | Default | Type | Description |
|---|---|---|---|
| `provider` | `"claude"` | string | `"claude"` (Claude Code CLI) or `"cursor"` (Cursor Agent CLI). |
| `cursor_cli` | `"agent"` | string | Cursor Agent executable name or absolute path. Only used when `provider = "cursor"`. |
| `cursor_model` | `"Auto"` | string | Cursor Agent `--model`. `"Auto"` (any case) or empty = automatic model selection. |
| `step_timeout_secs` | `1800` | int | Per-step timeout in seconds. Applies to all providers. |
| `model` | `""` | string | Model override (e.g. `"claude-opus-4-6"`, `"claude-sonnet-4-6"`). Empty = provider default. |

---

## `[docker]`

| Key | Default | Type | Description |
|---|---|---|---|
| `build_commands` | `[]` | array<string> | Optional `bash -c` steps run during image build (see `MAESTRO_BUILD_CONFIG` in `docker-compose.yml`). |
| `compose_up_commands` | `[]` | array<string> | Optional commands run as the `maestro` user after preflight on every `docker compose up`. Skills from `./skills` do not require a hook here — they are merged automatically. |

---

## `[editor]`

In-browser VS Code (openvscode-server) and dynamic port forwarding.

| Key | Default | Type | Description |
|---|---|---|---|
| `ports` | `[]` | array<int> | Application ports to expose when opening the editor (e.g. `[3000, 5173, 6006]`). Each is mapped to a host port from the DinD range 9100–9200 and shown on the workflow card. |
| `dynamic_ports` | `10` | int | Spare ports pre-allocated for automatic dev-server forwarding. When a new listening port is detected inside the editor, socat forwards traffic from a spare host port. `0` disables dynamic forwarding. |
| `theme` | `"vs-dark"` | string | VS Code colour theme. |
| `extensions` | `[]` | array<string> | Marketplace IDs to pre-install (e.g. `["esbenp.prettier-vscode", "dbaeumer.vscode-eslint"]`). |
| `settings` | `{}` | table | Free-form VS Code settings under `[editor.settings]` (e.g. `"editor.fontSize" = 14`). |

---

## `[terminal]`

Web terminal (ttyd) inside the editor container.

| Key | Default | Type | Description |
|---|---|---|---|
| `git_editor` | *(unset)* | string | apt package name (e.g. `"nano"`, `"vim"`, `"micro"`, `"helix"`) installed inside every editor container; `git config --global core.editor` is set so `git commit`, `git rebase -i`, etc. open this editor. |
| `setup_commands` | `[]` | array<string> | Shell commands run **once per editor container lifetime** (guarded by a marker file). Use for expensive one-time setup. `/etc/maestro/env` is sourced before each command. |
| `startup_commands` | `[]` | array<string> | Shell commands run **every time** a fresh editor container is created. Use for tools that should be verified or updated on each editor open (e.g. mise-managed runtimes). |

---

## `[network]`

| Key | Default | Type | Description |
|---|---|---|---|
| `extra_egress_hosts` 🔒 | `[]` | array<string> | Domains added to the egress allowlist on top of the built-in defaults (Atlassian, GitHub, Anthropic, npm registry, private registries detected in `.npmrc`). Each entry expands the attack surface — only add hosts you trust the agent to talk to. |
| `allow_all_https` 🔒 | `false` | bool | When `true`, skip the egress allowlist and permit all outbound HTTPS (ports 443/8443). `extra_egress_hosts` is ignored when this is enabled. **Use only in trusted environments** — this disables one of Maestro's core mitigations. |

---

## `[dev]`

Dev-only knobs. Leave commented out in production.

| Key | Default | Type | Description |
|---|---|---|---|
| `mock_agent` | `false` | bool | When `true`, `ClaudeSession::run_prompt` and `CursorSession::run_prompt` short-circuit into a scripted mock session — no real `claude` or `agent` process spawns, no API tokens consumed. Honors the env override `MAESTRO_DEV_MOCK_AGENT=1` (env wins over config). Designed for E2E tests and dashboard demos. |
| `mock_agent_script_path` | `"tmp/mock_script.txt"` | string | Path to the mock script file. |
| `mock_agent_line_delay_ms` | `75` | int | Delay between mock output lines. |
| `mock_agent_total_ms` | `5000` | int | Total mock session duration. |

---

## `[provisioning]`

Extra CLI tools installed into the shared tools volume at startup, on top of
the ones baked into the image. Use this to pin a tool version, add a tool that
was removed from the bake, or skip one entirely.

| Key | Default | Type | Description |
|---|---|---|---|
| `install_commands` | `[]` | array of strings | Shell snippets run in order against the tools volume. The set is SHA-gated: Maestro hashes the canonicalised list and re-runs provisioning only when it changes. Each snippet should be idempotent (guard with `[ -f "$MAESTRO_TOOLS_BIN/<tool>" ] || …`) and install into `$MAESTRO_TOOLS_BIN`. |

---

## Ignored sections

The following sections are **parsed and ignored at load** — a startup warning is
logged so you notice. They moved out of `config.toml` because they are per-user
and per-workspace.

### `[commands]`

Worktree initialisation commands (formerly `pre_install`, `install`,
`pre_workflow`, now consolidated as `worktree_init_commands`) live in the
database, edited per workspace via the dashboard **Configuration → Worktree
Settings** tab or `PUT /api/admin/worktree-commands/{workspace}`. The deprecated
keys are still parsed and concatenated for backward compatibility; they will be
removed in **v0.8**.

### `[[run_commands]]`

Long-running dev-server commands surfaced as run/stop buttons in the dashboard
are per-user and configured via the same **Worktree Settings** tab.

---

## Environment variables

Secrets and per-host overrides go in `maestro.env` (mounted at
`/etc/maestro/env`). Only `export VAR=value` lines are honoured.

| Variable | Purpose |
|---|---|
| `ANTHROPIC_API_KEY` 🔒 | Claude API key (or use OAuth via `claude login`). |
| `ANTHROPIC_BASE_URL` | Override Anthropic API endpoint (proxy / on-prem gateway). |
| `CLAUDE_CODE_OAUTH_TOKEN` 🔒 | Token for Claude Code OAuth auth. |
| `CURSOR_API_KEY` 🔒 | Cursor Agent key — skips interactive `agent status` preflight. |
| `GH_TOKEN` 🔒 | GitHub PAT (fine-grained, scoped to the target repo). |
| `MAESTRO_CONFIG` | Path to an alternate `config.toml`. |
| `MAESTRO_DATA_DIR` | Override the persistent data directory (`maestro.db`, snapshots). |
| `MAESTRO_HOME` | Override the home base used to compute `MAESTRO_DATA_DIR`. |
| `MAESTRO_DEV_MOCK_AGENT` | `1` = force mock agent on (overrides `[dev] mock_agent`). |

See the [README's environment variables section](../README.md#environment-variables) for the
full `maestro.env` template.
