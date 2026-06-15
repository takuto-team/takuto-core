# Onboarding wizard v2 — requirements and acceptance criteria

## Problem statement

The first-login wizard (`ui/src/pages/Onboarding.tsx`) currently covers four steps: Ticketing, AI provider, GitHub integration, and Workflows. Three config areas with meaningful first-run defaults are absent from the wizard — git base branch/remote, step timeout, and item-polling policy — so operators discover them only after workflows fail or behave unexpectedly. Additionally, starting the app for the first time requires the operator to manually write a `config.toml` before the container will even start; the wizard cannot create that file for them.

This spec covers four targeted enhancements that address these gaps.

---

## Enhancement 1 — Git settings in the GitHub step

### Decision: relabel the existing step

Step 3 is renamed from **"GitHub integration"** to **"Git & GitHub"**. The step gains a git settings section rendered *above* the existing GitHub App status and PAT fields. There is no new wizard step; the step count stays at four.

The step title string in `ui/src/pages/Onboarding/Stepper.tsx` (index 3, currently `"GitHub integration"`) is updated to `"Git & GitHub"`, and the step body copy is updated to reflect the expanded scope.

The two new fields map to `crates/takuto-core/src/config/git.rs`:

| Field | Config key | Default |
|---|---|---|
| Base branch | `git.base_branch` | `"main"` |
| Remote | `git.remote` | `"origin"` |

Both fields are editable by any authenticated user who reaches the wizard; `PUT /api/config` (or the git-specific sub-route, whichever the team wires) saves them. The fields are pre-populated from the value returned by `GET /api/config` at wizard load time (same pattern as the existing provider form via `useProviderForm`).

### Acceptance criteria

**AC-G1** — Given the wizard is at step 3, the step header reads "Git & GitHub" (not "GitHub integration") and the stepper pill updates to match.

**AC-G2** — Given `GET /api/config` returns `git.base_branch = "main"` and `git.remote = "origin"`, both input fields are pre-populated with those values when step 3 renders.

**AC-G3** — Given an operator changes base branch to `"develop"` and remote to `"upstream"` and clicks Continue, `PUT /api/config` (or the relevant sub-route) is called with `git.base_branch = "develop"` and `git.remote = "upstream"` before the wizard advances.

**AC-G4** — Given the operator clicks "Skip for now" on step 3, no save call is made and the wizard advances with the existing config unchanged.

**AC-G5** — Given the operator clears the base branch field and attempts to Continue, the UI blocks the save and shows an inline validation message (field is required).

**AC-G6** — The existing GitHub App status panel and personal access token section (`GitHubCredentialsSection`) remain fully functional below the new git fields, in the same visual order as before.

---

## Enhancement 2 — Item-polling settings in the Ticketing step

### Decision: component reuse; non-admins see nothing

The existing `ItemPollingSettingsSection` component (`ui/src/components/admin/ItemPollingSettingsSection.tsx`) is embedded at the bottom of the **Ticketing** wizard step (step 1), rendered only when both conditions hold:

1. The **currently selected** ticketing system is `"github"` or `"jira"` (not `"none"`).
2. The **current user is an admin** (`isAdmin === true` from the auth context).

This mirrors exactly the gate in `TicketingTab.tsx`:
```ts
const showPolling = !loading && !!isAdmin && ticketing.system !== "none";
```

Non-admins never see the polling section in the wizard, matching the Config tab behaviour. The existing `PUT /api/config/polling` 403 enforcement on the server remains the authoritative security boundary.

The polling section saves independently (via its own internal Save button inside `ItemPollingSettingsSection`) and does not block wizard navigation — the Continue / Skip buttons for step 1 save only the ticketing system selection, exactly as today.

### Acceptance criteria

**AC-P1** — Given an admin selects "None" on step 1, the item-polling section is not rendered.

**AC-P2** — Given an admin selects "GitHub" or "Jira" on step 1, the item-polling section renders below the ticketing selector with the same content as the Config → Ticketing tab.

**AC-P3** — Given a non-admin (any role other than admin) completes step 1 with Jira or GitHub selected, the item-polling section is not rendered at any point.

**AC-P4** — Given an admin edits a polling field and clicks the Save button inside the polling section, `PUT /api/config/polling` is called and a success toast is shown; the wizard step navigation is unaffected.

**AC-P5** — Given an admin clicks "Skip for now" or "Continue" on step 1 without clicking the polling section's Save button, any unsaved polling changes are discarded and no extra API call is made for polling.

**AC-P6** — Given the admin switches the ticketing selector from "GitHub" to "None" and back to "GitHub", the polling section disappears and reappears accordingly (reactive to the live selection, not requiring a save first — same behaviour as `TicketingTab`).

---

## Enhancement 3 — Step timeout in the Workflows step

A step-timeout input is added to wizard step 4 (Workflows), rendered **above** the `FlowsTab` component. It maps to `crates/takuto-core/src/config/agent.rs`:

| Field | Config key | Default |
|---|---|---|
| Step timeout (seconds) | `agent.step_timeout_secs` | `1800` |

The field is pre-populated from `GET /api/config` at wizard load time. Saving on Continue calls `PUT /api/config/agent` with the updated `step_timeout_secs` value.

### Acceptance criteria

**AC-T1** — Given `GET /api/config` returns `agent.step_timeout_secs = 1800`, the timeout input on step 4 shows `1800` when the step renders.

**AC-T2** — Given an operator changes the timeout to `3600` and clicks "Finish setup", `PUT /api/config/agent` is called with `step_timeout_secs = 3600` before the wizard completion call is made.

**AC-T3** — Given the operator enters a non-positive value or clears the field, the UI blocks save and shows an inline validation message (must be a positive integer).

**AC-T4** — Given the operator clicks "Skip for now" on step 4, no agent-config save call is made and the wizard advances.

**AC-T5** — The `FlowsTab` component renders below the timeout field and remains fully functional.

---

## Enhancement 4 — config.toml bootstrap (no-file-needed first run)

### Behaviour

When `config.toml` does not exist on first start:

1. The Rust server starts normally using `Config::default()` (already the case — `crates/takuto-cli/src/main.rs:634–638`).
2. The dashboard detects first-run state (no config file) via the existing `GET /api/onboarding/status` mechanism and **automatically navigates to `/onboarding`** before the user can reach the main dashboard.
3. At the end of the wizard ("Finish setup"), a `POST /api/onboarding/complete` call triggers the server to write a complete `config.toml` via the existing `ConfigWriter` (`crates/takuto-core/src/config_writer.rs`). The file is built from the settings the operator entered during the wizard merged with `Config::default()` for everything else.
4. The written `config.toml` is a complete, valid file. Fields not collected by the wizard (database connection, dashboard port, etc.) are written with their `Config::default()` values (SQLite at the default data-dir path, port 8080) so the operator has a complete file to edit afterwards.
5. The `docker/entrypoint.sh:445` hard-fail block (`exit 1` when config.toml is absent) is removed or made conditional on a new `--require-config` flag. On normal Docker startup the container must now boot without a config file.

### Notes on out-of-scope fields

`database.connection` and `web.port` are startup-only settings that cannot be changed via a running wizard (the server is already bound to a port and connected to a DB). The wizard does **not** collect them. The written `config.toml` contains sensible defaults for both. The wizard's Finish screen includes a brief copy note:

> **Note:** Database and dashboard port are not configured in this wizard. Edit `config.toml` directly to change them — the defaults (SQLite, port 8080) are written for you.

### Acceptance criteria

**AC-B1** — Given no `config.toml` exists at the configured path, the server starts, serves the dashboard, and the dashboard redirects unauthenticated users to the first-user setup page (existing flow); authenticated users are redirected to `/onboarding`.

**AC-B2** — Given the operator completes all four wizard steps and clicks "Finish setup", `config.toml` is written to the configured path (default `config.toml` for local, `/etc/takuto/config.toml` for Docker) and contains a `[git]`, `[agent]`, `[general]`, `[database]`, and `[web]` section with no empty required fields.

**AC-B3** — Given `config.toml` is written by the wizard, subsequent server restarts load that file normally (`Config::load` path, not `Config::default`).

**AC-B4** — Given the operator skips all wizard steps without changing any values and clicks "Finish setup", a valid `config.toml` is still written with all-default values.

**AC-B5** — Given `config.toml` already exists when the server starts, the wizard auto-launch does not trigger (existing `onboarding_complete` state is respected).

**AC-B6** — Given the Docker container starts without a `config.toml` volume mount, the container does not exit with code 1; it starts, serves the wizard, and writes `config.toml` on wizard completion (the `docker/entrypoint.sh` hard-fail block must be removed or bypassed for this path).

**AC-B7** — The written `config.toml` includes a comment block at the top (or inline) noting that `database.connection` and `web.port` must be edited manually, and showing the current default values.

---

## Out of scope

The following were explicitly excluded and must **not** be added to the wizard:

- **Database step** — `database.connection` is a startup-only field. The wizard must not collect it. The default (SQLite) is written silently.
- **Dashboard port / General step** — `web.port` is startup-only. Same rule.
- Any new wizard steps beyond the existing four.

---

## Open questions for the team

**OQ-1 (Backend)** — Is there a dedicated `PUT /api/config/git` sub-route, or should git fields be patched via the generic `PUT /api/config`? The spec uses `PUT /api/config` as a placeholder; the backend team should confirm the correct endpoint and update the frontend spec accordingly.

**OQ-2 (Backend)** — On wizard completion, should `POST /api/onboarding/complete` be extended to accept the full config diff in its body (and perform the write server-side), or should each wizard step save its own config slice as the user navigates, with `POST /api/onboarding/complete` only writing the final file and marking onboarding done? The latter is closer to the current pattern.

**OQ-3 (Frontend)** — The `useProviderForm` hook already loads `GET /api/config` for step 2; the new git fields and step-timeout field need the same data. Confirm whether a single shared config-load hook can serve steps 2, 3, and 4 without duplicate fetches, or whether each step continues to fetch independently.

**OQ-4 (Backend/Docker)** — The entrypoint `exit 1` guard for missing config.toml should be removed for the no-config bootstrap path. Confirm whether any provisioning logic in `entrypoint.sh` before line 445 depends on config.toml being present (e.g. `[provisioning].install_commands` at line 166), and document any sequencing constraint.
