# Implement-Workflow E2E Contract

Source-confirmed contract for the **implement-workflow** e2e suite: agent-CLI
reachability (Part A) and a real **opencode** workflow against a mock LM Studio,
driving the interactive surface — init `npm ci`, run-commands, IDE, terminal,
dynamic port-forward + `/s/` proxy (Part B). Every fact carries an inline
`file:line`. If the source changes, update this file in the same task.

Companion to `e2e/CONTRACT.md` (onboarding/registration). Reuse its app-boot,
registration, and `GET /api/config` facts; this doc covers only the
implement-workflow surface.

---

## 0. The four facts that unblock the team

| # | Fact | Value |
|---|------|-------|
| 1 | **Tools dir / enable install** | `TAKUTO_TOOLS_DIR` (default `/opt/takuto-tools`). The startup install runs **iff that directory exists**; absent → skipped (local-dev no-op). Binaries land in `<dir>/bin`, which is first on container `PATH`. |
| 2 | **Mock `/v1/chat/completions` response** | Mock must serve OpenAI **non-streaming** `POST /v1/chat/completions` → `200` JSON with `choices[0].message.content` = some text + `finish_reason:"stop"`. That is all opencode needs to finish a step with exit 0. **`[dev] mock_agent` must be OFF** or opencode is short-circuited (§2.4). |
| 3 | **Interactive routes** | `PUT /api/worktree-commands/{workspace}` (init + run + report), `POST /api/workflows/{id}/run-workflow/{def}`, run-commands `…/start` `…/stop`, `POST …/open-editor`, `POST …/open-terminal`; events over `GET /ws`; proxied via `/s/{path_token}/…`. (§3) |
| 4 | **React fixture layout** | A committed Vite+React app (with lockfile) at the local repo root; registered with `ticketing_system = none` + `git.repo_path` → the fixture. Workflow started via the paste-description modal. (§4) |

---

## 1. Part A — Agent-CLI install & reachability

### Install trigger & gating
| Fact | Value | Source |
|------|-------|--------|
| Startup install entry | `dependency_status::spawn_install(config)` called at server boot | `crates/takuto-cli/src/server/mod.rs:66` |
| Install dir | env `TAKUTO_TOOLS_DIR` → default `/opt/takuto-tools`; binaries in `<dir>/bin` | `crates/takuto-web/src/dependency_status.rs:106-107`; `agent_install.rs:198-204` |
| **Enable/skip rule** | runs **only if `install_dir` exists**; absent → logs "Tools volume absent" and returns (Idle) | `dependency_status.rs:108-114` |
| Runs in background | `tokio::spawn` → `Installer::install_all(&cfg, &GlobalSink)` (server keeps serving) | `dependency_status.rs:122-129` |
| Status probe | `GET /api/…` dependency-install status snapshot (`Phase::Installing`→`Ready`/`Error`) | `server.rs:570-572`; `dependency_status.rs:80-99` |
| Container image pre-creates dir | `/opt/takuto-tools` + `/bin` created & chmod 0755 in the image | `Dockerfile:365-378` |
| CLI-mode install (alt path) | `takuto agents install` → `Installer::install_all(&cfg, &StdoutSink)` | `crates/takuto-cli/src/commands/agents.rs:31-32` |

### What gets installed (`specs_from_config`)
The set = agents in `[agent].available_providers` (**defaults to all four**) **plus
`acli` always** (ticketing can switch at runtime). `agent_install.rs:122-172`.

| Provider | `name` | **Binary** (`<dir>/bin/…`) | Install kind | Version probe |
|----------|--------|----------------------------|--------------|---------------|
| claude | Claude Code | **`claude`** | npm `@anthropic-ai/claude-code` | `claude --version` |
| codex | Codex | **`codex`** | npm `@openai/codex` | `codex --version` |
| opencode | OpenCode | **`opencode`** | npm `opencode-ai` | `opencode --version` |
| cursor | Cursor Agent | **`agent`** (+ **`cursor-agent`** symlink) | HTTPS tarball | `agent --version` |
| (always) | Atlassian CLI | **`acli`** | HTTPS binary | `acli --version` |

- Binary names: `agent_install.rs:130,134/141,148/154,160-161` (cursor symlinks both
  `agent` and `cursor-agent` → `cursor-agent` launcher: `agent_install.rs:320-321`).
- **Version subcommand is `--version` for all five** (`detect_version`:
  `agent_install.rs:207-224`). `parse_version` takes the first digit-leading token
  (`agent_install.rs:93-102`), e.g. `2.1.178 (Claude Code)` → `2.1.178`.
- Unpinned (`version=""`) → always reinstall **latest**; pinned+match → **Skip**
  (`plan_one`: `agent_install.rs:79-87`).

### Part A pass conditions
For each of `claude`, `agent` (cursor), `opencode`, `codex`: the binary exists on
`PATH` (resolves by bare name from `/opt/takuto-tools/bin`) **and** `<' bin '> --version`
exits 0 and prints a version token parseable by `parse_version`. (acli optional for
this suite but installed regardless.) Wait for the dependency-install status to reach
`Ready` (`Phase::Ready`) before asserting, since install is async.

---

## 2. Part B — opencode workflow against mock LM Studio

### 2.1 How opencode reaches the mock (base_url honoured)
opencode has **no env-var fallback**; the bundle materialises an `opencode.json`
that defines a single `provider.self_hosted` using the Vercel AI-SDK
`@ai-sdk/openai-compatible` adapter pointed at the admin `base_url`.
`crates/takuto-core/src/auth/bundle/opencode_config.rs:38-201`.

| Field | Value | Source |
|-------|-------|--------|
| File name | `opencode.json` (in the bundled config dir, mounted `~/.config/opencode:ro`) | `opencode_config.rs:38-39` |
| Provider id | **`self_hosted`** (verbatim in `-m self_hosted/<model>`) | `opencode_config.rs:52` |
| Adapter | `npm = "@ai-sdk/openai-compatible"` | `opencode_config.rs:57,168` |
| Endpoint | `provider.self_hosted.options.baseURL` = admin `base_url` | `opencode_config.rs:173-179` |
| API key | `options.apiKey` = user bearer, else dummy **`lm-studio`** | `opencode_config.rs:46,129-142` |
| Model key | `provider.self_hosted.models.<modelId>` (provider prefix stripped) | `opencode_config.rs:116-124,164-165` |

`base_url` + `model` are **required** (blank → `400 opencode_base_url_required` /
`opencode_model_required` at config save; see `e2e/CONTRACT.md §3 step 3`). Set both
in onboarding so the workflow can run. The AI-SDK appends `/v1/chat/completions` to
`baseURL` — point `base_url` at the mock's `/v1` root (e.g.
`http://mock-lmstudio:1234/v1`).

### 2.2 The request opencode (AI SDK) sends to the mock
Standard **OpenAI Chat Completions, non-streaming**:
- `POST {baseURL}/chat/completions` (i.e. `…/v1/chat/completions`)
- Headers: `Authorization: Bearer <apiKey>`, `Content-Type: application/json`
- Body: `{ "model": "<modelId>", "messages": [ {role, content}, … ], … }`
  (the AI-SDK openai-compatible adapter; `stream` defaults to false for `generateText`).

### 2.3 The minimal mock response that completes a step
Return `200 application/json`:
```json
{
  "id": "chatcmpl-mock",
  "object": "chat.completion",
  "created": 0,
  "model": "<echo the requested model>",
  "choices": [
    { "index": 0,
      "message": { "role": "assistant", "content": "Done." },
      "finish_reason": "stop" }
  ],
  "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
}
```
Why this is sufficient: opencode wraps the turn and writes its **own** NDJSON to
stdout (`opencode run --format json`). The step succeeds when the `opencode` process
**exits 0** with **non-empty stdout** and **no `type:"error"` event**; a non-empty
assistant `content` yields a `type:"text"` event so the parser returns text.
`crates/takuto-core/src/opencode/session.rs:136-193`. Any non-2xx / malformed mock
response makes opencode emit a `type:"error"` event → step fails
(`session.rs:148-165,177-183`).

Serve a single `POST /v1/chat/completions`. Also answer `GET /v1/models` benignly if
queried (optional). Keep it **non-streaming** (no SSE).

### 2.4 opencode invocation shape (`build_opencode_args`)
`crates/takuto-core/src/opencode/session.rs:205-256`:
```
opencode run --format json --dangerously-skip-permissions \
  --print-logs --log-level WARN \
  [-m self_hosted/<model>] [-s <session_id>] "<prompt>"
```
- Model always prefixed `self_hosted/` if not already (`session.rs:233-239`).
- Resume uses **`-s <id>`** (not `--resume`) (`session.rs:242-249`).
- Prompt is the **last positional** arg (`session.rs:251-253`).
- Spawned via `ProcessHandle::spawn[_with_env]` in the worktree dir; container path
  wraps with `ContainerRunner::wrap_command("opencode", …)` (`session.rs:121-128`).

> **Critical:** the dev mock short-circuits opencode entirely — if `[dev] mock_agent`
> / `TAKUTO_DEV_MOCK_AGENT=1` is set, `run_prompt` returns a stub without spawning the
> binary (`session.rs:64-69`). For Part B the dev mock **must be OFF** so the real
> `opencode run` executes against the mock LM Studio.

---

## 3. Interactive surface (REST + WS + `/s/` proxy)

### 3.1 Worktree commands — `PUT /api/worktree-commands/{workspace}`
| Item | Value | Source |
|------|-------|--------|
| Handler | `put_my_row` | `crates/takuto-web/src/routes/worktree_commands.rs:407` |
| Router | registered | `crates/takuto-web/src/server.rs:436-438` |
| Body (`deny_unknown_fields`) | `init_commands: Vec<String>`, `run_commands: Vec<RunCommand>`, `generate_report: bool` (all `#[serde(default)]`) | `worktree_commands.rs:96-105` |
| `RunCommand` shape | `{ name, command }` | `crates/takuto-core/src/db/user_worktree_commands.rs` |

`init_commands` run sequentially in the worktree during bootstrap (each a step
`Worktree init (i/n): …`). For the fixture set `init_commands = ["npm ci"]`.
`run_commands` are long-running dev servers started on demand (§3.3).

### 3.2 Start a flow — `POST /api/workflows/{id}/run-workflow/{def}`
| Item | Value | Source |
|------|-------|--------|
| Handler | `run_workflow_def` → `202 Accepted` (409 on failure) | `crates/takuto-web/src/routes/workflows/definitions.rs:42` |
| Router | registered | `server.rs:303-304` |

`{def}` = the flow slug (kebab-cased flow name). First run on a Pending card
bootstraps (worktree create + init commands) then runs the flow's agent/command steps.

### 3.3 Run-commands (dev servers) — start / stop
| Item | Value | Source |
|------|-------|--------|
| Start | `POST /api/workflows/{id}/run-commands/{index}/start` → `start_run_command` | `server.rs:275-276`; `routes/workflows/run_commands.rs:80` |
| Stop | `POST /api/workflows/{id}/run-commands/{index}/stop` → `stop_run_command` | `server.rs:283-284`; `run_commands.rs:330` |
| `RunCommandStatus` | `{ index: usize, name: String, running: bool, forwarded_port: Option<(u16 container_port, String proxy_url)> }` | `routes/workflows/dto.rs:146-158` |
| WS `run_command_port_forwarded` | emitted when a started run-command's port is detected & forwarded | `routes/workflows/port_tracking.rs:144-204` |

### 3.4 Open IDE — `POST /api/workflows/{id}/open-editor`
| Item | Value | Source |
|------|-------|--------|
| Handler / router | `open_editor` | `routes/workflows/editor.rs:45`; `server.rs:235-240` |
| `OpenEditorResponse` | `{ url, connection_token, vscode_port: u16, port_mappings: Vec<(u16,u16)>, path_token }` | `editor.rs:29-42` |
| `url` shape | `/s/<path_token>/?tkn=<connection_token>&folder=<…>` | `editor.rs:31-32` |

### 3.5 Open terminal — `POST /api/workflows/{id}/open-terminal`
| Item | Value | Source |
|------|-------|--------|
| Handler / router | `open_terminal` | `editor.rs:868`; `server.rs:251-256` |
| `OpenTerminalResponse` | `{ url, credential, path_token }` | `editor.rs:743-754` |
| `url` shape | `/s/<path_token>/<ttyd-token>/` | `editor.rs:744-745` |

### 3.6 Dynamic port-forward WS event
| Item | Value | Source |
|------|-------|--------|
| Event carrier | `WorkflowEvent` (serde JSON, `event_type` discriminator) | `crates/takuto-core/src/workflow/engine/types.rs:75-129` |
| Port payload | `forwarded_port: Option<(u16 container_port, u16 host_port)>` | `types.rs:95-97` |
| `event_type` values | `"port_forwarded"` (editor), `"run_command_port_forwarded"` (run-command) | `types.rs:77`; `port_tracking.rs:144` |

### 3.7 `/s/{path_token}/…` shared-port reverse proxy
| Item | Value | Source |
|------|-------|--------|
| Route | `.route("/s/{*rest}", any(proxy_session))` | `server.rs:509` |
| Handler | `proxy_session` / `proxy_or_static_fallback` | `routes/sessions/mod.rs:187,71` |
| Registry | `PathTokenRegistry` (`Arc<RwLock<HashMap<token, SessionRoute>>>`); `lookup(token)` → `SessionRoute { kind, host_port, ticket_key, user_id }` | `session_registry.rs:83-200,94-96` |
| HTTP forward | upstream `http://127.0.0.1:{host_port}/{rest}` | `routes/sessions/proxy_forward.rs` |
| WS forward | bidirectional tunnel, 101 Upgrade | `routes/sessions/websocket.rs` |
| Auth | requires valid `takuto_session` cookie before token lookup | `routes/sessions/mod.rs:187-202` |

A path_token registered by open-editor / open-terminal / a forwarded run-command port
maps to a loopback `host_port`; `/s/<token>/…` proxies there. Specs hit the proxied
`url` from the open-editor / open-terminal / port-forward responses.

### 3.8 WebSocket events — `GET /ws`
| Item | Value | Source |
|------|-------|--------|
| Route | `.route("/ws", get(ws_handler))` | `server.rs:499` |
| Handler | subscribes to `engine.subscribe()` broadcast | `routes/ws.rs:14-39,55` |
| Message | `Message::Text` = `serde_json::to_string(&WorkflowEvent)` | `routes/ws.rs:76-79` |
| Filtering | per-socket: event delivered when `user_id == None` or matches viewer | `routes/ws.rs:47-52` |

Connect with the session cookie; filter messages on `event_type` /
`forwarded_port` (§3.6) to observe a step completing and ports forwarding.

---

## 4. Vite React app fixture & registration

### 4.1 Minimal fixture (committed git repo)
A local git repo whose root is a runnable Vite + React app:

```
<fixture>/
  package.json          # scripts.dev = "vite"; deps react, react-dom;
                        # devDeps vite, @vitejs/plugin-react
  package-lock.json     # committed → `npm ci` works offline-deterministically
  vite.config.(j|t)s    # server.host = "0.0.0.0" (and a fixed server.port)
  index.html
  src/main.jsx          # ReactDOM.createRoot(...).render(<App/>)
  src/App.jsx           # trivial component rendering a known marker string
  .git/                 # committed: package*.json, vite config, index.html, src/
```

- `package.json` `scripts.dev = "vite"` (the run-command will be `npm run dev`).
- **Lockfile committed** so the init step `npm ci` is deterministic.
- `vite.config` **`server.host = "0.0.0.0"`** so the dev server binds beyond loopback
  (required for the container port to be detectable/forwardable); pin `server.port`.
- App must render a stable, assertable marker (e.g. a heading) so a spec can confirm
  the proxied URL serves the running app.

### 4.2 Registration without GitHub
- `[general] ticketing_system = none` — no Jira/GitHub poller; manual start only
  (AGENTS.md "Ticketing modes"). `GET /api/config` → `ticketing_system: "none"`.
- `[git] repo_path` → the fixture repo on disk. `workspace_name` is the **last path
  component** of `repo_path` (`workspace_name_from_repo_path()`), used to scope
  `PUT /api/worktree-commands/{workspace}` and `GET /api/workflows`.
- Start a workflow via the paste-description modal (ticketing=none): the **+** tile
  opens a 1000px modal; name → slugified `ticket_key` (drives branch/worktree),
  description → `ticket_description`. Then click the flow button →
  `POST /api/workflows/{id}/run-workflow/{def}`.
- No GitHub App / PAT needed: with no remote push step in the flow, bootstrap git
  author falls back to local config; the flow's steps are the opencode step + the
  fixture's command steps.

---

## 5. Acceptance criteria

### Part A — four CLIs reachable + respond
Precondition: tools dir exists (`/opt/takuto-tools` or `TAKUTO_TOOLS_DIR`), so
`spawn_install` runs; wait for dependency-install `Phase::Ready`.

| AC | Action | Pass condition |
|----|--------|----------------|
| A1 | resolve `claude` on PATH; `claude --version` | exit 0, version token parseable |
| A2 | resolve `agent` (cursor) on PATH; `agent --version` | exit 0, version token; `cursor-agent` symlink also resolves |
| A3 | resolve `opencode` on PATH; `opencode --version` | exit 0, version token |
| A4 | resolve `codex` on PATH; `codex --version` | exit 0, version token |

### Part B — opencode workflow + interactive surface
Precondition: onboarding completed with `provider = opencode`, `base_url` = mock
`…/v1`, `model` set; `ticketing_system = none`; `git.repo_path` = fixture;
`PUT /api/worktree-commands/{workspace}` sets `init_commands = ["npm ci"]` and one
`run_commands` entry `{ name:"dev", command:"npm run dev" }`; **dev mock OFF**.

| AC | Action | Pass condition |
|----|--------|----------------|
| B1 init `npm ci` ran | start the flow (paste modal → `run-workflow/{def}`) | a `Worktree init (1/1): npm ci` step appears and succeeds; `node_modules` present in worktree |
| B2 workflow reaches terminal state via mock | opencode step runs against mock | mock receives `POST /v1/chat/completions`; flow's def run → `Completed` (workflow not `Error`); WS shows step completion |
| B3 run-commands at end | `POST …/run-commands/{index}/start` for `dev` | `RunCommandStatus.running == true`; later `…/stop` → `running == false` |
| B4 IDE reachable | `POST …/open-editor`; GET the proxied `url` (with cookie) | `200` from `/s/<path_token>/…`; openvscode-server UI/asset served |
| B5 terminal reachable | `POST …/open-terminal`; GET the proxied `url` | `200` from `/s/<path_token>/<ttyd-token>/`; ttyd served |
| B6 dev server → port forwarded → proxied URL responds | start `dev` run-command (`npm run dev`); wait for `run_command_port_forwarded` | `RunCommandStatus.forwarded_port = Some((5173-ish, proxy_url))`; GET that proxy_url → `200` serving the fixture app's marker string |

> "terminal reachable" (B5) and "npm-run-dev from terminal" overlap with the
> run-command path (B6); the run-command start (`npm run dev`) is the canonical
> dev-server trigger. A spec may instead type `npm run dev` in the opened terminal
> (B5) and assert the same forwarded-port + proxy result (B6) — both exercise the
> dynamic port-forward + `/s/` proxy.

---

## 6. Notes for implementers
- Reuse `e2e/CONTRACT.md` for registration, `GET /api/config`, cookie, and config.toml
  facts; this doc adds only the implement-workflow surface.
- Container `PATH` resolves agent binaries by bare name from `/opt/takuto-tools/bin`
  (`docker/entrypoint.sh:198`, `Dockerfile:395`).
- Do **not** reference plan/slice/phase artifacts anywhere in test/app code.
