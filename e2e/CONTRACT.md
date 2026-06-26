# Onboarding E2E Contract

The canonical, source-confirmed contract the e2e harness, page objects, and specs
build against. Every fact below is verified against the working tree (file:line
references inline). If the source changes, update this file in the same task.

All UI selectors are stable `#onb-*` ids unless noted. Footer/stepper/registration
controls have no ids — target them via the documented role/text and centralize the
strings in a page object.

---

## 1. App boot facts

| Fact | Value | Source |
|------|-------|--------|
| HTTP port | `8080` | `Dockerfile:415` (`EXPOSE 8080`) |
| Health probe | `GET /api/health` → `200`, body `ok` (text/plain, **lowercase**) | `crates/takuto-web/src/server.rs:564-565`, mounted `.route("/health", …)` under `.nest("/api", api)` `server.rs:110,493` |
| No `config.toml` present | server boots on `Config::default()` and serves the wizard | (see §6) |
| DB backend selector | env `TAKUTO_DATABASE_CONNECTION` — empty→SQLite `${TAKUTO_DATA_DIR}/takuto.db`; `sqlite://…` / `postgres://…` / `postgresql://…` / `mysql://…` | `crates/takuto-cli/src/server/database.rs:50-58`; `crates/takuto-core/src/db/mod.rs:226-232` |
| Data dir | env `TAKUTO_DATA_DIR` → `$TAKUTO_HOME` → `$HOME`; holds `takuto.db`, `secret.key` | `crates/takuto-cli/src/server/database.rs:82-88` |
| Migrations | auto-run on boot, same set dialect-translated per backend | `crates/takuto-core/src/db/migrate.rs` |

---

## 2. First-admin registration flow (blank → admin → wizard)

A fresh app (no users, no `config.toml`) reaches the wizard like this:

1. **Probe** `GET /api/auth/status` (public) → `setup_required: true` when the DB has
   zero users. The SPA renders the **Setup** page (`ui/src/pages/Setup.tsx`) at `/`.
   - Response fields: `dashboard_auth_enabled`, `multi_user`, `setup_required`,
     `provider_selected`, `github_mode`, `degraded`, `provider_credential_present`
     (`crates/takuto-web/src/routes/auth/status.rs:12-33,88-96`).
2. **Register** `POST /api/auth/register` `{username, password}` →
   `201 Created` `{user_id, username, role:"admin", recovery_codes:[…], redirect_to:"/onboarding"}`.
   - Password must be **≥ 12 chars**; empty username rejected; first user becomes **admin**.
     Returns `409` once any user exists (`register.rs:44-119`, min-length `register.rs:72`).
   - **Does NOT set a session cookie.** The UI then auto-logins.
3. **Login** `POST /api/auth/login` `{username, password}` → `204 No Content` +
   `Set-Cookie: takuto_session=…` (HttpOnly, `SameSite=Lax`, `Secure` resolved from web
   cfg + request headers, `Max-Age` = idle TTL).
   - Cookie name constant: **`takuto_session`** (`crates/takuto-web/src/auth.rs:66`).
   - Handler: `crates/takuto-web/src/routes/auth/login.rs:49-178`.
4. UI navigates to `/onboarding` (`Setup.tsx:110-119`, `redirect_to` from step 2).

**Setup page selectors** (no `#onb-*` ids; `ui/src/pages/Setup.tsx`):
- Username: `input[autocomplete="username"]`
- Password: first `input[autocomplete="new-password"]`; Confirm: second one
- Submit: button text `Create account` (i18n `auth:setup.createAccount`)
- After 201 a **recovery-codes screen** appears: tick the acknowledge checkbox
  (`input[type=checkbox]`), then click `Continue` (`auth:setup.continueToDashboard`)
  → auto-login + `window.location.replace("/onboarding")`.

**Re-login (restart-persistence specs):** prefer the **API client** `POST /api/auth/login`
to obtain the `takuto_session` cookie. UI fallback (`ui/src/pages/Login.tsx`): username
`input[autocomplete="username"]`, password `input[type=password]`, submit `auth:login.signIn`.

---

## 3. Wizard step order, selectors, required fields, validation

Canonical order from `ONBOARDING_STEPS` (`ui/src/pages/Onboarding/Stepper.tsx:7-13`),
**1-indexed**, rendered by `ui/src/pages/Onboarding.tsx:206-290`:

| # | Step | Component | Saved on Continue (hook) |
|---|------|-----------|--------------------------|
| 1 | Git & GitHub | `GitHubStep` | per-user PAT then `PUT /api/config/git` (`useGitForm`) |
| 2 | Repositories | `MyRepositoriesTab` | nothing — own Add/Remove buttons |
| 3 | AI provider | `ProviderStep` + `OnboardingAiKey` | `PUT /api/config/agent` then per-user AI key |
| 4 | Ticketing | `TicketingStep` (+ polling) | `PUT /api/config` (system) + Jira cred + polling |
| 5 | Workflows | step-timeout + `FlowsTab` | flush flow edit then `PUT /api/config/agent` (step_timeout) |

`onBeforeNext` per-step logic: `Onboarding.tsx:120-159`. Returning `false` blocks advance.

### Step 1 — Git & GitHub (`ui/src/pages/Onboarding/GitHubStep.tsx`)
| Selector | Field | Required | Notes |
|----------|-------|----------|-------|
| `#onb-git-base-branch` | base branch (text) | **yes**, non-empty | seeds to `main`; `disabled` when caller not admin |
| `#onb-git-remote` | remote (text) | **yes**, non-empty | seeds to `origin`; `disabled` when not admin |
- GitHub App status: read-only line "GitHub App: configured / not configured".
- Per-user PAT panel (`GitHubCredentialsSection`, `deferSave`) — saved on Continue.
- Validation (`useGitForm.ts:68-84`): blank base branch → inline `git.baseBranchRequired`
  ("Base branch is required."); blank remote → `git.remoteRequired`; `save()` returns
  `false`, Continue blocked. Non-admin: inputs read-only, `save()` is a no-op that advances.

### Step 2 — Repositories (`ui/src/components/MyRepositoriesTab.tsx`)
- No `#onb-*` ids; adds/removes persist via their own buttons. Skippable; nothing on Continue.

### Step 3 — AI provider (`ui/src/pages/Onboarding/ProviderStep.tsx`)
| Selector | Field | Notes |
|----------|-------|-------|
| `#onb-provider` | provider `<select>` | options **`claude`, `cursor`, `codex`, `opencode`** (`ProviderStep.tsx:7`) |
| `#onb-base-url` | base URL (text) | **disabled + forced empty when provider = `cursor`** (`ProviderStep.tsx:31,65,72`) |
| `#onb-model` | model (text) | |
| `#onb-extra-args` | extra args (textarea, one per line) | |
- AI key panel: `OnboardingAiKey` (`ui/src/pages/Onboarding/OnboardingAiKey.tsx`) — saved on Continue.
- Client sends `base_url` omitted for cursor (`useProviderForm.ts:107-111`); `model` sent only when non-empty (`:115-117`).
- **OpenCode validation is server-side** (`PUT /api/config/agent` → `config.validate()`):
  blank `base_url` → `400` `opencode_base_url_required`; blank `model` → `400`
  `opencode_model_required` (base_url checked first) — `crates/takuto-core/src/config/load.rs:226-248`.
  The wizard surfaces the error as a toast and `save()` returns `false` → Continue blocked.
  OpenCode shows a red `*` on base URL + model labels.

### Step 4 — Ticketing (`ui/src/pages/Onboarding/TicketingStep.tsx`)
| Selector | Field | Notes |
|----------|-------|-------|
| `#onb-ticketing` | system `<select>` | options **`none`, `github`, `jira`** (`TicketingStep.tsx:8`); `disabled` when not admin |
| `#onb-jira-site` | Jira site (text) | only when system = `jira` |
| `#onb-jira-email` | Jira email (email) | only when `jira` |
| `#onb-jira-token` | Jira API token (password) | only when `jira`; masked + a `Replace` button when a credential already exists |
- Per-repo polling section (`RepoPollingSettingsSection`) renders when system ≠ `none`; saved on Continue.
- **Jira partial-form blocks Continue** (`useTicketingForm.ts:123,132-135`): when **not** already
  connected and 1–2 of {site,email,token} are filled (`filledCount>0 && <3`) → toast
  `ticketing.jiraPartial`, `save()` returns `false`. All-3 filled saves; all-blank is a no-op
  that advances (when system unchanged). Connected user may leave blank to keep the token.

### Step 5 — Workflows (`ui/src/pages/Onboarding.tsx:260-289`)
| Selector | Field | Required | Notes |
|----------|-------|----------|-------|
| `#onb-step-timeout` | step timeout secs (number, `min=1`) | **yes**, positive int | seeds to `1800` |
- Validation (`useStepTimeoutForm.ts:48-57`): blank or `≤ 0` → inline `stepTimeout.invalid`
  ("Step timeout must be a positive number."), `save()` returns `false`, Finish blocked.
- `FlowsTab` editor below; an open invalid draft blocks Finish (`Onboarding.tsx:150-156`).

### Footer & stepper (no ids — target by role/text)
- `WizardFooter` (`ui/src/components/WizardFooter.tsx`): **Back** button (i18n `onboarding:nav.back`
  = "← Back", disabled on step 1) and primary button whose label is:
  - steps 1–4: `nav.continue` = "Save and Continue" (→ `nav.saving` = "Saving…" while in flight)
  - step 5: `nav.finish` = "Finish setup" (→ `nav.finishing` = "Finishing…")
- Finish (step 5 primary) calls `POST /api/onboarding/complete` then navigates to `/`
  (`useOnboardingFlow.ts:30-53`).
- Header has a top-left **"Skip setup →"** link to `/` (`onboarding:header.skip`) and, when auth
  enabled, a logout button.
- `Stepper` (`Stepper.tsx`): `<nav aria-label>` → `<ol>` → `<li>`; the active step carries
  `aria-current="step"`; each `<li>` text is `"{n}. {title}"`.

---

## 4. Verification endpoints + exact serde field names

> **Correction to the plan:** there is **no** `GET /api/config/git` or `GET /api/config/agent`
> (both are **PUT-only** — `server.rs:373,383`). Read-back of git/agent/ticketing settings is
> the single flattened **`GET /api/config`**.

### `GET /api/onboarding/status` (public; `routes/onboarding.rs:91-209`)
Body = flattened `SystemStatus` + the per-user fields below (latter omitted when unauthenticated):
```
user_onboarding: {            // present only with a valid session cookie
  step_1_ticketing:  string|null,   // "completed" | "skipped" | null
  step_2_provider:   string|null,
  step_3_github:     string|null,
  step_4_credentials:string|null,   // auto-flips to "completed" when an active provider cred exists
  completed_at:      string|null    // <-- CANONICAL "wizard finished" signal (non-null = done)
}
jira_credential_present: bool
```
Field names: `routes/onboarding.rs:71-78`. The four `step_*` names are a DB completion model,
**not** the 5-step UI order — treat `completed_at` as the done signal; step flags are advisory.

### `GET /api/config` (auth required; `routes/config.rs:20-104`)
Flattened `Config` (secrets redacted via `redacted_for_api_clone`) + runtime flags. Fields the
specs assert:
| Path | Meaning |
|------|---------|
| `git.base_branch`, `git.remote` | git settings entered in step 1 |
| `agent.provider` | `"claude"`/`"cursor"`/`"codex"`/`"opencode"` |
| `agent.step_timeout_secs` | step-5 timeout |
| `agent.providers.<provider>.base_url` / `.model` / `.extra_args` | provider sub-table |
| `general.ticketing_system` | persisted ticketing system |
| `ticketing_system` (top-level) | runtime mirror: `"jira"`/`"github"`/`"none"` |
| `github_app_configured`, `jira_available`, `config_writable`, `repo_exists` | runtime flags |

### Completion: `POST /api/onboarding/complete` (auth-gated; `routes/onboarding.rs:240-271`)
- Marks `onboarding_state.completed_at` (best-effort) **and** does a full `config.toml` write
  via `ConfigWriter`. Response `{persisted: bool, persist_warning?: string}`.
- Unauthenticated → `401` (route is in the protected router).

### Per-user credential endpoints (`crates/takuto-web/src/routes/credentials.rs`, registered `server.rs:393-411`)
| Purpose | Method + path | Request body (serde) |
|---------|---------------|----------------------|
| AI provider key | `POST /api/users/me/credentials/{provider}` | `{ api_key?, claude_session_json?, kind?="api_key" }` |
| GitHub PAT | `POST /api/users/me/github-pat` | `{ pat, attribute_commits?=true }` |
| Jira credential | `POST /api/users/me/jira-credential` | `{ site, email, token? }` (omit `token` to keep stored) |
| Read-back (no secrets) | `GET /api/users/me/credentials[?provider=<p>]` | → `{ provider?, github?, jira? }` status objects |

Jira body field names confirmed against the UI client (`useTicketingForm.ts:147-151`: `{site,email,token?}`).

---

## 5. Per-step write endpoints (what each Continue persists)
| Step | Endpoint | Admin-gated? |
|------|----------|--------------|
| 1 git | `PUT /api/config/git` `{base_branch?, remote?}` (deny_unknown_fields; all-null→400; blank→400) | **yes** (403 non-admin) — `config_git.rs:50-95` |
| 3 provider | `PUT /api/config/agent` `{provider?, providers:{<p>:{base_url?,model?,extra_args?}}, step_timeout_secs?, …}` | **yes** — `config_agent.rs:254-309` |
| 4 ticketing system | `PUT /api/config` `{general:{ticketing_system}}` (deny_unknown_fields, strict allowlist) | **yes** — `config.rs:121-161` |
| 5 step timeout | `PUT /api/config/agent` `{step_timeout_secs}` (`Config::validate` enforces ≥1) | **yes** |

The first registered user is **admin**, so the e2e admin can drive every step.

---

## 6. config.toml path + master key

### config.toml write path (in container)
- Default container path: **`/etc/takuto/config.toml`** — `Dockerfile:444-445`:
  `ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]`, `CMD ["--config", "/etc/takuto/config.toml"]`.
- Resolution: `--config` / env `TAKUTO_CONFIG` (default `config.toml`) →
  `crates/takuto-cli/src/cli.rs`; entrypoint default `${TAKUTO_CONFIG:-/etc/takuto/config.toml}`
  (`docker/entrypoint.sh:209`). `/etc/takuto` is chowned to the runtime user so the writer can
  write at runtime.
- `ConfigWriter` writes atomically (temp+rename, in-place fallback on EBUSY, keeps `.bak`) —
  `crates/takuto-core/src/config_writer.rs`.
- Read it in specs via `docker exec <takuto-container> cat /etc/takuto/config.toml`; assert
  `[agent] provider`, `[general] ticketing_system`, `[git] base_branch` / `remote`,
  `[agent] step_timeout_secs`. After Finish the file always exists with SQLite + port-8080
  defaults even if every step was skipped (`routes/onboarding.rs:221-265`).

### TAKUTO_SECRET_KEY master key
- Format: **64-character hex string = 32 bytes**; optional `0x`/`0X` prefix; whitespace trimmed.
  Invalid length → `ConfigError` at parse; wrong length after decode → error.
  `crates/takuto-core/src/auth/master_key.rs:42-66`.
- Example value a test may pin:
  `0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef`
- Resolution chain (`master_key.rs:90-184`): env `TAKUTO_SECRET_KEY` → `${TAKUTO_DATA_DIR}/secret.key`
  (32 raw bytes, mode 0600) → auto-generate **only if** `[general] allow_auto_generate_secret_key = true`.
- **Restart-persistence requirement:** the fixture must **pin `TAKUTO_SECRET_KEY`** (or persist
  `secret.key` on the data volume) so encrypted credential rows decrypt after a container restart.

---

## 7. Acceptance criteria

### A. Happy-path completion + restart-persistence — full cartesian (run per backend project)
Providers × Ticketing × Backends = **4 × 3 × 3 = 36** happy + **36** restart cases.

| Axis | Values |
|------|--------|
| Provider (`#onb-provider`) | `claude`, `cursor`, `codex`, `opencode` |
| Ticketing (`#onb-ticketing`) | `none`, `github`, `jira` |
| Backend (`TAKUTO_DATABASE_CONNECTION`) | `sqlite`, `postgres`, `mysql` |

Per combination, **Happy**:
1. Fresh stack (no `config.toml`), pinned `TAKUTO_SECRET_KEY`.
2. Register admin → auto-login → reach `/onboarding`.
3. Walk all 5 steps entering provider/ticketing-specific values + dummy credentials where a
   panel exists. opencode → set base URL + model. cursor → base URL stays disabled. jira → fill
   all three Jira fields. Finish.
4. Assert `GET /api/onboarding/status` → `user_onboarding.completed_at` non-null.
5. Assert `GET /api/config` reflects: `agent.provider`, `agent.providers.<p>.base_url`/`model`
   (cursor: no base_url), `ticketing_system`, `git.base_branch`/`remote`, `agent.step_timeout_secs`.
6. Assert `config.toml` (docker exec) carries provider, ticketing_system, base_branch, remote,
   step_timeout_secs.

Per combination, **Restart-persistence**: after Happy, `stack.restart()` (same DB + same master
key) → re-login via API → assert the admin user still exists, `completed_at` still non-null,
`GET /api/config` unchanged, and stored credentials still present/decryptable
(`GET /api/users/me/credentials` shows the jira/provider/github rows).

### B. Validation cases — **run once per backend** (client/server logic, provider-invariant)
| Case | Action | Expected |
|------|--------|----------|
| git base branch required | clear `#onb-git-base-branch`, Continue | stays on step 1, inline `git.baseBranchRequired` |
| git remote required | clear `#onb-git-remote`, Continue | stays on step 1, inline `git.remoteRequired` |
| step timeout ≥ 1 | set `#onb-step-timeout` to `0`/blank, Finish | stays on step 5, inline `stepTimeout.invalid` |
| opencode requires base_url + model | provider `opencode`, blank base URL/model, Continue | stays on step 3 (server 400 `opencode_base_url_required` / `opencode_model_required`, toast) |
| cursor disables base_url | provider `cursor` | `#onb-base-url` disabled + empty; advancing persists no base_url |
| jira partial-form blocks Continue | system `jira`, fill 1–2 of site/email/token, Continue | stays on step 4, toast `ticketing.jiraPartial` |

### C. Skip cases — **run once per backend**
- Use the per-step skip paths / "Skip setup →" so steps record `skipped` (or stay null), then
  Finish; assert `POST /api/onboarding/complete` still succeeds and `completed_at` is set.
- A fully-skipped run still yields a valid `config.toml` (SQLite + port 8080 defaults).

> Matrix is env-configurable so devs can scope down locally (e.g. `TAKUTO_E2E_BACKENDS=sqlite`).
