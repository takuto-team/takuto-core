# Per-user Jira credential — architectural decision

Date: 2026-06-15
Status: accepted (drives the onboarding-wizard rework)

## Context

Until now Takuto authenticates to Jira through a **single, shared** Atlassian
CLI (`acli`) login established once per deployment:

- `docker/entrypoint.sh` runs `acli jira auth login --site <site> --email <email> --token`
  during `make setup` Step 2, reading `[jira] site` / `[jira] email` from `config.toml`.
- `JiraClient` (`crates/takuto-core/src/jira/client.rs`) and the `JiraPoller`
  shell out to that one authenticated `acli` binary.
- `docker_hooks::check_acli_auth()` probes the global auth state at startup and
  gates `jira_available`.

This means every ticket Takuto touches is assigned/transitioned as the **same**
Atlassian identity, regardless of which Takuto user created the workflow. That
contradicts the per-user isolation model already in place for workflows
(`Workflow.user_id`), GitHub PATs (`user_github_credentials`), and AI provider
keys (`user_provider_credentials`).

## Decision

Introduce a **per-user Jira credential** — `(site, email, API token)` — stored
encrypted per user, modeled exactly on the existing GitHub PAT credential:

- New sealed-credential table (e.g. `user_jira_credentials`), reusing the
  `auth::SealedBlob` envelope scheme (ciphertext + nonce + wrapped DEK + wnonce),
  exactly like `user_github_credentials`. Only the API token is sealed; `site`
  and `email` are plaintext metadata columns.
- New endpoints mirroring the GitHub PAT surface:
  `POST /api/users/me/jira-credential` (validate + seal + store),
  `DELETE /api/users/me/jira-credential` (wipe + audit row).
  Status is surfaced through the existing `GET /api/users/me/credentials` bundle.
- Save path validates the credential against Jira before storing (a cheap
  authenticated call, e.g. current-user / `myself`) and records `last_validated_at`,
  consistent with the GitHub PAT flow. Writes are co-committed with a
  `credential_audit` row in one transaction.

## Runtime consumption (poller + client)

- A workflow's Jira operations (assign, transition, fetch details) run with the
  **workflow owner's** Jira credential, resolved from `Workflow.user_id`.
- The **poller** uses the resolved poller-owner's credential (same owner
  resolution already used for `start_workflow`, see `resolve_poller_owner`).
  If the poller owner has no Jira credential, the poller logs a warning and is
  idle for Jira (consistent with today's `jira_available = false` fallback).
- `jira_available` becomes effectively **per-user / per-workflow**: a workflow
  whose owner has no valid Jira credential behaves as `jira_available = false`
  (Jira steps skipped), exactly as the existing flag already drives behavior.

## Implementation note (acli vs REST)

`acli` holds a single global auth state and cannot carry a per-call identity, so
per-user Jira auth cannot be expressed by re-running `acli auth login` per
request. The GitHub PAT path already calls the provider API directly with the
user's token; the Jira credential should follow the same shape — authenticate
each Jira call with the owner's `(email, token)` (Atlassian basic-auth against
`<site>/rest/api/...`). The exact client refactor (REST client vs. per-user
`acli` profile dir) is an implementation decision for the backend task; this
note only fixes the **credential ownership + storage model**.

## Consequences

- `[jira] site` / `[jira] email` in `config.toml` and the shared `acli` login
  remain valid as a deployment-level fallback during transition, but the
  per-user credential takes precedence when present.
- Onboarding step 1 now captures the credential at first login when the user
  selects Jira (see onboarding-wizard rework).
- `config.toml`'s ticketing section stops being the only place to pick the
  ticketing system: step 1 writes `[general] ticketing_system`.
