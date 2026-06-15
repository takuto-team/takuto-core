# Configuration reference

Canonical reference for every key in `config.toml`. Source of truth is
[`config.toml.example`](../config.toml.example) at the repository root — when
this page and that file disagree, the example wins.

Settings are reloaded on file change (5-second poller). A few keys are
**startup-only** and require a restart (noted inline). Secrets belong in
`takuto.env`, never in `config.toml`.

Takuto splits config into two kinds:

- **Bootstrap** — needed before the database/dashboard exist, or applied only at
  startup. Hand-edit these in `config.toml` before the first boot.
- **UI-managed** — has a sane default and is normally edited from a dashboard
  **Configuration** screen. The UI writes changes back to `config.toml`, so the
  file stays the store. A value set here is only a deploy-time default.

The **Scope** column below marks each key as **Bootstrap** or names the UI
surface (tab / endpoint) that manages it.

## Conventions

- 🔒 — security-sensitive. Review before changing in a shared deployment.
- **Default** column shows the value Takuto uses when the key is absent.
- **Scope** column: **Bootstrap** (hand-edit, often startup-only) or the UI tab /
  endpoint that manages the key.
- Commented-out keys in `config.toml.example` show their default in the comment;
  this page lists the same defaults explicitly.

## Table of contents

- [`[general]`](#general)
- [`[jira]`](#jira)
- [`[polling]`](#polling)
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

| Key | Default | Type | Scope | Description |
|---|---|---|---|---|
| `ticketing_system` | `"none"` | string | Bootstrap | `"jira"`, `"github"`, or `"none"`. Drives the poller and the dashboard **+** button. |
| `dry_mode` 🔒 | `false` | bool | Bootstrap | Skip real Jira/GitHub side effects (no assigns, transitions, or pushes). Local work — worktrees, installs, agent sessions — still runs. Useful for staging a workflow definition before going live. |
| `log_level` | `"info"` | string | Bootstrap (restart) | `trace`, `debug`, `info`, `warn`, `error`. Applied via `tracing_subscriber` `EnvFilter`. |
| `worker_image` | `""` | string | Bootstrap | Docker image for isolated workflow containers when DinD is enabled. Empty = auto-detect from the running Takuto image. |
| `workflow_definitions_dir` | `"workflows"` | string | Bootstrap (restart) | Path scanned for `*.toml` workflow definitions, resolved relative to the config file's directory. |
| `allow_auto_generate_secret_key` 🔒 | `true` | bool | Bootstrap | When `true`, the server auto-generates `{data_dir}/secret.key` on first boot if neither `TAKUTO_SECRET_KEY` nor a keyfile is present. Set `false` to provision the key out of band (boots degraded until provided). |
| `poller_owner_username` | *(unset)* | string | Bootstrap | Username of the user who owns workflows created automatically by the poller. When unset, the lexicographically-first non-suspended admin is used. When set but missing/suspended, an admin fallback applies with a warning. When neither resolves, the poller skips entirely. |
| `migrate_orphan_workflows` | `false` | bool | Bootstrap | When `true`, restored workflows with `user_id = None` (pre-multi-user orphans) are reassigned to the resolved poller owner at startup. |
| `migrate_orphan_repo_associations` | `true` | bool | Bootstrap | When `true`, startup reconciliation creates a `user_repositories` association for every restored snapshot workflow that has a `user_id` and whose `workspace_name` matches a registered repository. Set to `false` to require users to re-add the repository explicitly. |
| `auto_polling` | `true` | bool | UI: Item Polling | When `false`, polling starts paused at boot (same as clicking **Pause polling** on the dashboard). Use **Resume polling** to pick up new tickets. |
| `poll_interval_secs` | `60` | int | UI: Item Polling | Seconds between Jira/GitHub polls. |
| `pr_merge_poll_interval_secs` | `60` | int | UI: Item Polling | Seconds between polls that check whether a workflow's PR has been merged on GitHub. `0` disables. |
| `max_concurrent_workflows` | `1` | int | UI: Item Polling | Semaphore size for parallel mise/install/agent sessions. Tune to your server's CPU and RAM. |
| `max_active_workflows` | `0` | int | UI: Item Polling | Max workflows visible on the dashboard (every row including **Done**, **Paused**, **Stopped**, **Error**). The poller will not start new tickets while at this limit. `0` mirrors `max_concurrent_workflows`. |
| `max_concurrent_manual_workflows` | `0` | int | UI: Item Polling | Cap on dashboard **+** manual starts that are not **Done**/**Stopped**/**Error**. `0` = no limit. The **+** tile is disabled when full. |
| `generate_report` | `false` | bool | UI: Item Polling | When `true`, each agent step appends findings to `lore/reports/<item-key>_report.md` and a final consolidation step produces a polished summary. Adds tokens — leave `false` unless you want the **Report** button populated. |
| `work_item_log_retention_days` | `7` | int | UI: Item Polling | Days of work-item log lines to retain before the cleanup task deletes them. `0` = keep forever. |

---

## `[jira]`

Only used when `[general] ticketing_system = "jira"`.

| Key | Default | Type | Scope | Description |
|---|---|---|---|---|
| `site` | `""` | string | Bootstrap | Jira host or full `https://` URL (e.g. `"yourcompany.atlassian.net"`). Empty = `jira.atlassian.net`. Drives token auth, egress allowlist, and PR-body Jira link. |
| `email` | `""` | string | Bootstrap | Jira user email for `acli` token auth. |
| `project_keys` | `[]` | array<string> | UI: Item Polling | Jira project keys to poll (e.g. `["PROJ", "ENG"]`). The dashboard **+** picker is hidden when this is empty. |
| `item_types` | `["Task", "Bug"]` | array<string> | UI: Item Polling | Ticket types the poller picks up. The manual **+** picker ignores this and shows all non-Epic issues. |
| `done_status` | `"Done"` | string | UI: Item Polling | Transition name fired by **Mark as Done**. Must match a real transition in your Jira workflow. |
| `jql_filter` | `""` | string | UI: Item Polling | Extra JQL `AND`-merged into the dashboard **+** ticket picker (e.g. a board filter fragment). Epics are always excluded. |
| `linked_items_in_prompt` | `"full"` | string | UI: Item Polling | `"full"` \| `"summary_only"` \| `"omit"` — how linked-issue text appears in `{ticket_context}`. |
| `ticket_context_max_description_bytes` | `100000` | int | UI: Item Polling | Cap on the main ticket description in `{ticket_context}`. `0` = unlimited. |
| `linked_issue_description_max_bytes` | `32000` | int | UI: Item Polling | Cap per linked-issue description when `linked_items_in_prompt = "full"`. `0` = unlimited. |
| `acli_allowed_extra_prefixes` | `[]` | array<string> | Bootstrap | Advanced: extra allowed `acli` argv prefixes (tokens per line). |

---

## `[polling]`

Admin-tunable item-polling policy: which discovered work items the Jira/GitHub
pollers auto-add, which flow they auto-start, and how many items run in parallel.
Read **live** by the pollers each cycle (no restart). Editable from the admin
**Configuration → Item Polling** tab or `PUT /api/config/polling`.

All `[polling]` keys are **UI-managed** from Configuration → Item Polling.

| Key | Default | Type | Scope | Description |
|---|---|---|---|---|
| `auto_start_flow` | `""` | string | UI: Item Polling | Slug of the single flow to auto-start for each polled item. Empty = start **all** dependency-free flows (legacy behaviour). A slug that matches no dependency-free flow logs a warning and starts **nothing** (the item is still added to the dashboard). |
| `max_parallel_items` | `0` | int | UI: Item Polling | Cap on items occupying a concurrency slot at once. `0` = unlimited. The poller takes the tighter of this and the legacy `max_active_workflows`; manual starts enforce it as an **independent** `409` ceiling alongside `max_concurrent_manual_workflows`. |
| `max_parallel_per_user` | `false` | bool | UI: Item Polling | When `true`, `max_parallel_items` is counted per workflow owner; `false` counts it globally. |

Jira item **types** are configured under [`[jira] item_types`](#jira), not
duplicated here (the `PUT /api/config/polling` endpoint can patch that field too).

### `[polling.jira]`

| Key | Default | Type | Scope | Description |
|---|---|---|---|---|
| `summary_keywords` | `[]` | array<string> | UI: Item Polling | Case-insensitive ANY-substring match against the ticket summary. Empty = no filter. Blank/whitespace-only entries are rejected at save time. |

### `[polling.github]`

| Key | Default | Type | Scope | Description |
|---|---|---|---|---|
| `labels` | `[]` | array<string> | UI: Item Polling | Exact label membership, ANY match (an issue is kept if it carries any listed label). Empty = no filter. Blank entries rejected at save time. |
| `title_keywords` | `[]` | array<string> | UI: Item Polling | Case-insensitive ANY-substring match against the issue title. Empty = no filter. Blank entries rejected at save time. |

---

## `[git]`

All `[git]` keys are **bootstrap** settings.

| Key | Default | Type | Scope | Description |
|---|---|---|---|---|
| `base_branch` | `"main"` | string | Bootstrap | Branch each worktree is created from. |
| `remote` | `"origin"` | string | Bootstrap | Git remote used for fetch, base ref, and push. |
| `repo_path` | `"/workspace"` | string | Bootstrap | Path to the cloned repo inside the container. The workspace name is derived from this path's last component. |

---

## `[github]`

Optional GitHub App authentication. When configured, commits and PRs are attributed
to `takuto-bot[bot]` instead of the personal `gh` user.

All three fields (`app_id`, `app_installation_id`, and one private key source)
must be set together. Errors are non-fatal at startup — Takuto falls back to
personal `gh` auth and logs a warning.

Required App permissions: contents (write), pull_requests (write), metadata (read).

All `[github]` keys are **bootstrap** settings.

| Key | Default | Type | Scope | Description |
|---|---|---|---|---|
| `app_id` | *(unset)* | int | Bootstrap | GitHub App ID. |
| `app_installation_id` | *(unset)* | int | Bootstrap | App installation ID for your org/repo. |
| `app_private_key` 🔒 | *(unset)* | string | Bootstrap | PEM-encoded RSA private key, inline. Mutually exclusive with `app_private_key_path`. Prefer keeping this in `takuto.env` if you can — see [Environment variables](#environment-variables) in the README. |
| `app_private_key_path` 🔒 | *(unset)* | string | Bootstrap | Path to a PEM file with the App's private key. |
| `gh_allowed_extra_prefixes` | `[]` | array<string> | Bootstrap | Extra `gh` argv prefixes allowed beyond the built-in set (`api`, `pr create`, `pr edit`, `auth login`, `auth setup-git`, `auth status`). Each entry is a whitespace-separated token line. Set to `["*"]` to disable the gh allowlist entirely. |

---

## `[web]`

Dashboard server.

| Key | Default | Type | Scope | Description |
|---|---|---|---|---|
| `host` | `"0.0.0.0"` | string | Bootstrap (restart) | Bind address. |
| `port` | `8080` | int | Bootstrap (restart) | Bind port. |
| `cors_origins` 🔒 | `[]` | array<string> | Bootstrap (restart) | Allowed CORS origins. Empty = auto-compute from `host` and `port` (e.g. `http://localhost:8080`). Set explicitly when behind a reverse proxy or TLS terminator (e.g. `["https://takuto.example.com"]`). |
| `cookie_secure` 🔒 | *(auto-detect)* | bool | Bootstrap | Controls the `Secure` flag on the session cookie. Unset: auto-detect (set when any `cors_origins` entry is `https://…` OR when the inbound request carries `X-Forwarded-Proto: https`). Force `true` when terminating TLS in front of Takuto without `X-Forwarded-Proto`; use `false` for local plain-HTTP testing only. |
| `kick_other_sessions_on_login` | `true` | bool | Bootstrap | When `true`, a successful login deletes all prior sessions for the same user (single-session enforcement). Set `false` for long-lived multi-client sessions (desktop + mobile). |

> Authentication is **multi-user only**. On first boot the dashboard prompts you
> to create the initial admin account (`POST /api/auth/register`). The legacy
> single-user keys `[web] dashboard_username` / `dashboard_password` have been
> removed — if present in an old `config.toml` they are silently ignored.

**Session policies (not configurable):**

- Idle session TTL: 24 hours.
- Absolute session TTL: 30 days.
- Account lockout: after 5 failed login or recovery attempts in 10 minutes (admin clears via `POST /api/users/{id}/unlock`).
- Per-IP rate limit on `/api/auth/login` and `/api/auth/recover`: 10 / minute.

---

## `[agent]`

AI provider for `prompt`-bearing workflow steps. Every `[agent]` key is
**UI-managed** from Configuration → AI Settings; the values here are deploy-time
defaults.

| Key | Default | Type | Scope | Description |
|---|---|---|---|---|
| `provider` | `"claude"` | string | UI: AI Settings | `"claude"`, `"cursor"`, `"codex"`, or `"opencode"`. Each provider reads its own `[agent.providers.<name>]` sub-table. |
| `step_timeout_secs` | `1800` | int | UI: AI Settings | Per-step timeout in seconds. Applies to all providers. |
| `share_conversation_across_steps` | `false` | bool | UI: AI Settings | Share one agent conversation across a flow's steps (each step resumes the previous step's session) vs. a fresh session per step. |
| `max_repeated_output_lines` | `8` | int | UI: AI Settings | No-progress guardrail. Abort an agent step when its humanized output repeats the same substantive line this many times in a row (a stuck model retrying a failing action) — the step fails into **Error** instead of churning until `step_timeout_secs`. Session-lifecycle lines (`… initialized`/`completed`) are excluded so healthy multi-turn runs aren't tripped. `0` disables. |

Per-provider settings (model, endpoint, CLI path, extra args) live in
`[agent.providers.<name>]` sub-tables, all editable from Configuration → AI
Settings. Common fields: `model`, `extra_args`, `allow_shared_default`; Claude/Codex/OpenCode use `base_url`, Cursor uses `cli` instead.

### `[agent.providers.cursor]`

Sub-table for the Cursor Agent adapter. Only used when `provider = "cursor"`.

| Key | Default | Type | Scope | Description |
|---|---|---|---|---|
| `cli` | `"agent"` | string | UI: AI Settings | Cursor Agent executable name or absolute path. |
| `model` | `"Auto"` | string | UI: AI Settings | Cursor Agent `--model`. `"Auto"` (any case) or empty = automatic model selection. |
| `extra_args` | `[]` | array<string> | UI: AI Settings | Extra CLI flags (deny-list enforced). |

### `[agent.providers.opencode]`

Sub-table for the OpenCode (self-hosted, OpenAI-compatible) adapter. Only used when `provider = "opencode"`. All fields are editable from Configuration → AI Settings.

| Key | Default | Type | Scope | Description |
|---|---|---|---|---|
| `model` | `""` | string | UI: AI Settings | Model id served by the endpoint (the `<model>` in `-m self_hosted/<model>`). Required when active. |
| `base_url` | `""` | string | UI: AI Settings | OpenAI-compatible endpoint URL (e.g. `http://lm-studio:1234/v1`). Required when active. |
| `extra_args` | `[]` | array<string> | UI: AI Settings | Extra CLI flags (deny-list enforced). |
| `allow_shared_default` | `false` | bool | UI: AI Settings | Let users without a personal bearer fall back to the deployment-default token. |
| `context_limit` | `32768` | int | UI: AI Settings | Max context window (tokens) of the self-hosted model. Written to `models.<id>.limit.context` in the generated `opencode.json` so OpenCode tracks remaining context (it can't look this up for a local endpoint). Defaults to a 7B-class coder window; match your server's loaded context length, or clear from the dashboard to let OpenCode guess. |
| `output_limit` | `8192` | int | UI: AI Settings | Max output (tokens) per response. Written to `models.<id>.limit.output`. Defaults to a 7B-class coder window; clear from the dashboard to let OpenCode guess. |

---

## `[docker]`

All `[docker]` keys are **bootstrap** settings (consumed at image build / compose up).

| Key | Default | Type | Scope | Description |
|---|---|---|---|---|
| `build_commands` | `[]` | array<string> | Bootstrap | Optional `bash -c` steps run during image build (see `TAKUTO_BUILD_CONFIG` in `docker-compose.yml`). |
| `compose_up_commands` | `[]` | array<string> | Bootstrap | Optional commands run as the `takuto` user after preflight on every `docker compose up`. Skills from `./skills` do not require a hook here — they are merged automatically. |

---

## `[editor]`

In-browser VS Code (openvscode-server) and dynamic port forwarding.

All `[editor]` keys are **bootstrap** settings.

| Key | Default | Type | Scope | Description |
|---|---|---|---|---|
| `ports` | `[]` | array<int> | Bootstrap | Application ports to expose when opening the editor (e.g. `[3000, 5173, 6006]`). Each is mapped to a host port from the DinD range 9100–9200 and shown on the workflow card. |
| `dynamic_ports` | `10` | int | Bootstrap | Spare ports pre-allocated for automatic dev-server forwarding. When a new listening port is detected inside the editor, socat forwards traffic from a spare host port. `0` disables dynamic forwarding. |
| `theme` | `"vs-dark"` | string | Bootstrap | VS Code colour theme. |
| `extensions` | `[]` | array<string> | Bootstrap | Marketplace IDs to pre-install (e.g. `["esbenp.prettier-vscode", "dbaeumer.vscode-eslint"]`). |
| `settings` | `{}` | table | Bootstrap | Free-form VS Code settings under `[editor.settings]` (e.g. `"editor.fontSize" = 14`). |

---

## `[terminal]`

Web terminal (ttyd) inside the editor container.

All `[terminal]` keys are **bootstrap** settings.

| Key | Default | Type | Scope | Description |
|---|---|---|---|---|
| `git_editor` | *(unset)* | string | Bootstrap | apt package name (e.g. `"nano"`, `"vim"`, `"micro"`, `"helix"`) installed inside every editor container; `git config --global core.editor` is set so `git commit`, `git rebase -i`, etc. open this editor. |
| `setup_commands` | `[]` | array<string> | Bootstrap | Shell commands run **once per editor container lifetime** (guarded by a marker file). Use for expensive one-time setup. `/etc/takuto/env` is sourced before each command. |
| `startup_commands` | `[]` | array<string> | Bootstrap | Shell commands run **every time** a fresh editor container is created. Use for tools that should be verified or updated on each editor open (e.g. mise-managed runtimes). |

---

## `[network]`

All `[network]` keys are **bootstrap** settings (firewall policy — deploy-time only).

| Key | Default | Type | Scope | Description |
|---|---|---|---|---|
| `extra_egress_hosts` 🔒 | `[]` | array<string> | Bootstrap | Domains added to the egress allowlist on top of the built-in defaults (Atlassian, GitHub, Anthropic, npm registry, private registries detected in `.npmrc`). Each entry expands the attack surface — only add hosts you trust the agent to talk to. |
| `allow_all_https` 🔒 | `false` | bool | Bootstrap | When `true`, skip the egress allowlist and permit all outbound HTTPS (ports 443/8443). `extra_egress_hosts` is ignored when this is enabled. **Use only in trusted environments** — this disables one of Takuto's core mitigations. |

---

## `[dev]`

Dev-only knobs. Leave commented out in production.

All `[dev]` keys are **bootstrap** settings.

| Key | Default | Type | Scope | Description |
|---|---|---|---|---|
| `mock_agent` | `false` | bool | Bootstrap | When `true`, `ClaudeSession::run_prompt` and `CursorSession::run_prompt` short-circuit into a scripted mock session — no real `claude` or `agent` process spawns, no API tokens consumed. Honors the env override `TAKUTO_DEV_MOCK_AGENT=1` (env wins over config). Designed for E2E tests and dashboard demos. |
| `mock_agent_script_path` | `"tmp/mock_script.txt"` | string | Bootstrap | Path to the mock script file. |
| `mock_agent_line_delay_ms` | `75` | int | Bootstrap | Delay between mock output lines. |
| `mock_agent_total_ms` | `5000` | int | Bootstrap | Total mock session duration. |

---

## `[provisioning]`

Extra CLI tools installed into the shared tools volume at startup, on top of
the ones baked into the image. Use this to pin a tool version, add a tool that
was removed from the bake, or skip one entirely.

`[provisioning]` is a **bootstrap** setting (applied at startup).

| Key | Default | Type | Scope | Description |
|---|---|---|---|---|
| `install_commands` | `[]` | array of strings | Bootstrap | Shell snippets run in order against the tools volume. The set is SHA-gated: Takuto hashes the canonicalised list and re-runs provisioning only when it changes. Each snippet should be idempotent (guard with `[ -f "$TAKUTO_TOOLS_BIN/<tool>" ] || …`) and install into `$TAKUTO_TOOLS_BIN`. |

---

## Ignored sections

The following sections are **fully ignored at load** — a startup warning is
logged so you notice, but their contents are never read. They moved out of
`config.toml` because they are per-user and per-workspace. There is **no config
fallback**: if a section is absent from the database, the workflow simply runs no
commands for it.

### `[commands]`

Worktree initialisation commands (formerly `pre_install`, `install`,
`pre_workflow`, now consolidated as `worktree_init_commands`) live **only** in the
database, resolved per `(user, workspace)` and edited from the dashboard
**Configuration → Worktree Settings** tab. Any `[commands]` table — including the
deprecated `pre_install` / `install` / `pre_workflow` keys — in `config.toml` is
ignored entirely (logged, not concatenated, no global default).

### `[[run_commands]]`

Long-running dev-server commands surfaced as run/stop buttons in the dashboard
are per-user and configured via the same **Worktree Settings** tab.

---

## Environment variables

Secrets and per-host overrides go in `takuto.env` (mounted at
`/etc/takuto/env`). Only `export VAR=value` lines are honoured.

| Variable | Purpose |
|---|---|
| `ANTHROPIC_API_KEY` 🔒 | Claude API key (or use OAuth via `claude login`). |
| `ANTHROPIC_BASE_URL` | Override Anthropic API endpoint (proxy / on-prem gateway). |
| `CLAUDE_CODE_OAUTH_TOKEN` 🔒 | Token for Claude Code OAuth auth. |
| `CURSOR_API_KEY` 🔒 | Cursor Agent key — skips interactive `agent status` preflight. |
| `GH_TOKEN` 🔒 | GitHub PAT (fine-grained, scoped to the target repo). |
| `TAKUTO_CONFIG` | Path to an alternate `config.toml`. |
| `TAKUTO_DATA_DIR` | Override the persistent data directory (`takuto.db`, snapshots). |
| `TAKUTO_HOME` | Override the home base used to compute `TAKUTO_DATA_DIR`. |
| `TAKUTO_DEV_MOCK_AGENT` | `1` = force mock agent on (overrides `[dev] mock_agent`). |

See the [README's environment variables section](../README.md#environment-variables) for the
full `takuto.env` template.
