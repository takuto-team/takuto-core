# Maestro - Product Requirements Document

## 1. Product Overview

**Maestro** is an automated Jira ticket handler that orchestrates Claude Code headless sessions inside a Docker container to implement software tickets end-to-end — from picking up a Jira ticket in "To Do" to creating a pull request on GitHub.

### 1.1 Goals

- **Automate developer workflow**: Eliminate manual effort between ticket pickup and PR creation.
- **Ensure quality**: Multi-pass implementation with code review, linting, and testing built into the pipeline.
- **Provide visibility**: Real-time web dashboard showing workflow status, errors, and execution reports.
- **Maintain safety**: Docker isolation with allowlist-only egress; dry mode for risk-free testing.
- **Be configurable**: All project-specific settings (branches, commands, keys) are user-configurable without code changes.

### 1.2 Tech Stack

- **Language**: Rust (backend, workflow orchestrator, web server)
- **Container**: Docker with allowlist-only egress
- **External tools**: `acli` (Jira CLI), `gh` (GitHub CLI), `figma-cli`, Claude Code CLI, Playwright CLI
- **Skills**: Uses `/address-ticket` and `/review-changes` Claude Code skills

---

## 2. User Stories

### 2.1 Ticket Polling and Pickup

**US-2.1.1** As a user, I want Maestro to automatically poll Jira for tickets in "To Do" status so that new work is picked up without manual intervention.

**Acceptance Criteria:**
- Maestro polls Jira at a configurable interval (default: 60 seconds) using `acli`.
- Only tickets matching the configured project keys AND item types (e.g., Task, Bug) are picked up.
- Tickets are picked up one at a time, in creation-date order (oldest first).
- A ticket is only picked up if no workflow is currently running for it.
- In dry mode: polling occurs normally, but no ticket state changes are made.

**US-2.1.2** As a user, I want picked-up tickets to be assigned to the logged-in user and moved to "In Progress" so that the team has visibility into what Maestro is working on.

**Acceptance Criteria:**
- On pickup, the ticket is assigned to the Jira user authenticated via `acli`.
- The ticket status transitions from "To Do" to "In Progress".
- If assignment or transition fails, the ticket is skipped and the error is logged and displayed on the dashboard.
- In dry mode: assignment and status transition are skipped; the workflow proceeds with a log entry noting the skip.

### 2.2 Ticket Details Retrieval

**US-2.2** As a user, I want Maestro to retrieve the full ticket details including linked Jira items so that Claude Code has full context for implementation.

**Acceptance Criteria:**
- The ticket summary, description, acceptance criteria, and all custom fields are retrieved.
- Linked Jira items (blocks, is-blocked-by, relates-to, etc.) from configured project keys are retrieved recursively (one level deep).
- Linked item details include summary, description, and status.
- The assembled context is passed to the Claude Code session as the initial prompt.

### 2.3 Git Worktree Creation

**US-2.3** As a user, I want Maestro to create an isolated git worktree for each ticket so that parallel ticket work does not interfere.

**Acceptance Criteria:**
- A new git worktree is created from the configured base branch.
- Branch naming convention: `feat/<TICKET-KEY>` for Tasks/Stories, `fix/<TICKET-KEY>` for Bugs.
- The branch name is lowercase and kebab-case.
- If the branch already exists (e.g., from a previous attempt), the existing worktree is reused.
- On workflow stop or failure, the worktree is preserved (not automatically deleted) so that work-in-progress is not lost.

### 2.4 Claude Code Implementation Sessions

**US-2.4.1** As a user, I want Maestro to run Claude Code in headless mode with the `/address-ticket` skill so that the ticket is implemented automatically.

**Acceptance Criteria:**
- Claude Code is started with `--allow-dangerously-skip-permissions` flag.
- The working directory is set to the ticket's worktree.
- The `/address-ticket` skill is invoked with the full ticket context.
- A PM agent within the Claude Code session auto-confirms implementation plans by validating them against the ticket requirements and acceptance criteria.
- The session output (stdout/stderr) is captured and available for the execution report.

**US-2.4.2** As a user, I want Maestro to run the `/review-changes` skill after each implementation pass so that code quality issues are caught and fixed.

**Acceptance Criteria:**
- After `/address-ticket` completes, a new Claude Code session runs `/review-changes`.
- Review findings are addressed within the same session.
- The session is closed after review findings are resolved.

**US-2.4.3** As a user, I want Maestro to perform 3 total implementation passes so that the solution is iteratively refined.

**Acceptance Criteria:**
- The cycle is: `/address-ticket` -> `/review-changes` -> `/address-ticket` -> `/review-changes` -> `/address-ticket` -> `/review-changes`.
- That is: 3 rounds of (address-ticket + review-changes).
- Each pass builds on the previous one (same worktree, cumulative commits).
- If any session fails (non-zero exit, timeout, or crash), the error is captured and the workflow moves to the next step (linting) with whatever code exists.

### 2.5 Linting

**US-2.5** As a user, I want Maestro to run the configured lint command and fix any issues so that the PR meets code style requirements.

**Acceptance Criteria:**
- The configured lint command is run in the worktree.
- If linting fails, a Claude Code headless session is started to fix the lint errors.
- The fix-and-rerun cycle repeats until linting passes or a maximum of 3 attempts is reached.
- If linting passes (or max attempts reached), all changes are committed with a descriptive message.
- In dry mode: linting runs normally (it is a read+write local operation, not an external write).

### 2.6 Unit Tests

**US-2.6** As a user, I want Maestro to run unit tests and fix failures so that the implementation is verified.

**Acceptance Criteria:**
- The configured unit test command is run in the worktree.
- If tests fail, a Claude Code headless session is started to fix the failures.
- The fix-and-rerun cycle repeats until tests pass or a maximum of 3 attempts is reached.
- If tests pass (or max attempts reached), all changes are committed.
- Test output (pass/fail counts, failure details) is captured for the execution report.

### 2.7 End-to-End Tests

**US-2.7** As a user, I want Maestro to run e2e tests and fix failures so that the implementation works in a realistic environment.

**Acceptance Criteria:**
- The configured e2e test command is run in the worktree.
- If tests fail, a Claude Code headless session is started to fix the failures.
- The fix-and-rerun cycle repeats until tests pass or a maximum of 3 attempts is reached.
- If tests pass (or max attempts reached), all changes are committed.
- E2e test output is captured for the execution report.

### 2.8 Pull Request Creation

**US-2.8** As a user, I want Maestro to create a GitHub pull request so that the implementation is ready for human review.

**Acceptance Criteria:**
- The PR is created via `gh pr create`.
- PR title follows conventional commit format: `feat(TICKET-KEY): <ticket summary>` for Tasks/Stories, `fix(TICKET-KEY): <ticket summary>` for Bugs.
- PR description includes:
  - Jira ticket reference (link to ticket).
  - Summary of changes (generated from commit messages).
  - Test results summary (pass/fail counts for unit and e2e).
  - A note that the PR was auto-generated by Maestro.
- The PR targets the configured base branch.
- In dry mode: PR creation is skipped; a log entry records what would have been created.

### 2.9 Workflow Stop / Cancellation

**US-2.9** As a user, I want to stop a running workflow so that I can take over manually or abandon the work.

**Acceptance Criteria:**
- Stopping a workflow kills all running Claude Code sessions for that ticket.
- The Jira ticket is unassigned (Maestro's user removed as assignee).
- The ticket status transitions back to "To Do".
- The git worktree and branch are preserved.
- In dry mode: Jira changes are skipped; worktree is still preserved.

### 2.10 Workflow Pause / Resume

**US-2.10.1** As a user, I want to pause a running workflow so that I can temporarily halt Maestro without losing progress.

**Acceptance Criteria:**
- Pausing suspends execution after the current step completes (does not interrupt a running Claude Code session mid-execution).
- The workflow state is preserved: current step, all session outputs, all commits.
- The dashboard card reflects "Paused" status.
- The Jira ticket remains assigned and "In Progress".

**US-2.10.2** As a user, I want to resume a paused workflow so that it continues from where it left off.

**Acceptance Criteria:**
- Resuming continues from the next step after the one that was completed when pause was triggered.
- No work is duplicated or lost.

---

## 3. Web UI Requirements

### 3.1 Dashboard Page

The dashboard is the main page of the Maestro web UI. It displays all active, paused, and recently completed workflows.

**US-3.1.1** As a user, I want to see a card for each running/paused workflow so that I can monitor progress at a glance.

**Acceptance Criteria:**
- Each card displays:
  - Jira ticket key and summary (e.g., `PROJ-123: Add user login`).
  - Current workflow step (e.g., "Address Ticket - Pass 2 of 3", "Running Lint", "Creating PR").
  - Step progress indicator showing completed / total steps.
  - Status badge: "Running", "Paused", "Completed", "Failed".
  - Elapsed time since workflow started.
  - Error summary (if any step failed): short message with expand option.
- Cards are ordered by start time (newest first).

**US-3.1.2** As a user, I want Pause, Resume, and Stop buttons on each workflow card so that I can control workflows.

**Acceptance Criteria:**
- "Pause" button is visible when status is "Running". Clicking it pauses the workflow (see US-2.10.1).
- "Resume" button is visible when status is "Paused". Clicking it resumes the workflow (see US-2.10.2).
- "Stop" button is visible when status is "Running" or "Paused". Clicking it triggers a confirmation dialog, then stops the workflow (see US-2.9).
- Buttons are disabled while the action is being processed (prevent double-clicks).

**US-3.1.3** As a user, I want a "Report" button on each workflow card so that I can view a detailed execution report.

**Acceptance Criteria:**
- "Report" button is visible on all cards (including completed/failed workflows).
- Clicking it opens a modal with the execution report (see Section 3.3).

### 3.2 Configuration Page

**US-3.2** As a user, I want a configuration page so that I can adjust Maestro's settings without editing files.

**Acceptance Criteria:**
- All configurable items (see Section 5) are presented in a form.
- Changes are validated before saving:
  - Base branch: must be a valid git branch name.
  - Jira project keys: must be non-empty, uppercase alphanumeric.
  - Item types: at least one must be selected.
  - Lint/test/e2e commands: must be non-empty strings if enabled.
  - Poll interval: must be a positive integer (seconds), minimum 10.
  - Figma API token: optional, no format validation.
- Changes take effect on the next poll cycle (no restart required).
- Current values are shown as defaults in the form.
- A "Save" button persists changes. A "Reset to Defaults" button restores factory settings.

### 3.3 Report Modal

**US-3.3** As a user, I want to view a detailed execution report for any workflow so that I can understand what Maestro did and debug issues.

**Acceptance Criteria:**
- The report modal displays:
  - Ticket key, summary, and link to Jira.
  - Branch name and PR link (if created).
  - Timeline of all steps with start/end timestamps and status (success/failure/skipped).
  - For each Claude Code session: the prompt sent and a summary of the output.
  - Lint results: command run, pass/fail, number of fix attempts.
  - Unit test results: command run, pass/fail counts, number of fix attempts.
  - E2e test results: command run, pass/fail counts, number of fix attempts.
  - Final commit log (all commits made during the workflow).
  - Errors: full error messages with stack traces where available.
- The modal is scrollable and has a close button.
- Content is text-selectable for copying.

---

## 4. Dry Mode Specification

Dry mode allows running the full pipeline without making any writes to external systems (Jira, GitHub). It is intended for testing and demonstration.

### 4.1 Behavior

| Action | Normal Mode | Dry Mode |
|---|---|---|
| Poll Jira for tickets | Executes | Executes |
| Assign ticket | Executes | **Skipped** (logged) |
| Move to In Progress | Executes | **Skipped** (logged) |
| Retrieve ticket details | Executes | Executes |
| Create worktree | Executes | Executes |
| Run Claude Code sessions | Executes | Executes |
| Lint / Test / E2e | Executes | Executes |
| Commit changes | Executes | Executes |
| Create PR | Executes | **Skipped** (logged) |
| Push branch | Executes | **Skipped** (logged) |
| On stop: unassign ticket | Executes | **Skipped** (logged) |
| On stop: move to To Do | Executes | **Skipped** (logged) |

### 4.2 Dry Mode Indicators

- The dashboard displays a prominent banner: "DRY MODE - No external writes" when dry mode is active.
- Each skipped action in the execution report is marked with a "[DRY]" prefix.
- The configuration page shows the current dry mode status.

---

## 5. Configuration Schema

All configuration is stored in a single file (`maestro.toml` at the project root or a path specified by `MAESTRO_CONFIG` env var).

| Key | Type | Default | Description |
|---|---|---|---|
| `base_branch` | string | `"main"` | Git branch to create worktrees from |
| `jira_project_keys` | string[] | `[]` | Jira project keys to poll (e.g., `["PROJ", "CORE"]`) |
| `jira_item_types` | string[] | `["Task", "Bug"]` | Jira issue types to pick up |
| `lint_command` | string | `""` | Command to run for linting (empty = skip) |
| `unit_test_command` | string | `""` | Command to run for unit tests (empty = skip) |
| `e2e_test_command` | string | `""` | Command to run for e2e tests (empty = skip) |
| `poll_interval_secs` | u64 | `60` | Seconds between Jira polls |
| `dry_mode` | bool | `false` | Enable dry mode |
| `figma_api_token` | string | `""` | Figma API token for design references |
| `max_fix_attempts` | u32 | `3` | Max attempts to fix lint/test failures per step |
| `max_concurrent_workflows` | u32 | `1` | Maximum concurrent ticket workflows |
| `web_port` | u16 | `8080` | Port for the web dashboard |
| `session_timeout_secs` | u64 | `1800` | Timeout for each Claude Code session (30 min default) |

---

## 6. Authentication and Container Setup

### 6.1 Required Tools in Docker Image

The Docker image must include:
- `acli` (Atlassian CLI for Jira)
- `gh` (GitHub CLI)
- `figma-cli`
- Claude Code CLI
- Playwright CLI (for e2e tests)
- Git
- Rust toolchain (for building Maestro itself)
- Node.js runtime (for Claude Code and Playwright)
- The project's Claude Code skills collection

### 6.2 Authentication Flows

| Tool | Auth Method | Setup |
|---|---|---|
| `acli` | API token | `ACLI_API_TOKEN` env var, configured at container start |
| `gh` | OAuth or PAT | `GH_TOKEN` env var, or `gh auth login` with token at container start |
| `figma-cli` | API token | `FIGMA_API_TOKEN` env var (also stored in `maestro.toml`) |
| Claude Code | API key | `ANTHROPIC_API_KEY` env var |

All tokens are passed via environment variables at container startup. No interactive login flows are required.

### 6.3 Network Egress Allowlist

The Docker container restricts outbound traffic to only the following hosts:

| Host | Purpose |
|---|---|
| `*.atlassian.net` | Jira API via acli |
| `api.github.com` | GitHub API via gh |
| `github.com` | Git push/pull |
| `api.figma.com` | Figma API |
| `api.anthropic.com` | Claude API |
| `npm.registry.npmjs.org` | npm packages (for Playwright, etc.) |
| `registry.npmjs.org` | npm registry |

All other outbound connections are blocked.

---

## 7. Error Handling and Edge Cases

### 7.1 Step Failures

- If a Claude Code session crashes or times out, the workflow continues to the next step with whatever code exists.
- If linting/testing fails after max fix attempts, the workflow continues to the next step. The failure is logged and visible in the report.
- If PR creation fails, the error is logged. The branch and commits remain intact for manual PR creation.

### 7.2 Jira Errors

- If a ticket cannot be assigned or transitioned (e.g., permission error, workflow restriction), the ticket is skipped. The error is logged on the dashboard.
- If Jira is unreachable during a poll, the poll is skipped and retried at the next interval. A warning is shown on the dashboard.

### 7.3 Git Errors

- If worktree creation fails (e.g., branch conflict, disk space), the ticket is skipped with an error logged.
- If a commit fails, the error is logged but the workflow continues (subsequent steps may still generate changes).

### 7.4 Concurrent Access

- Only one workflow runs per ticket (enforced by in-memory tracking).
- `max_concurrent_workflows` limits total parallel workflows. Additional tickets wait in the queue.
- Configuration changes via the web UI are applied atomically (no partial updates).

### 7.5 Container Restart

- On container restart, all in-flight workflows are lost (no persistent state).
- Tickets that were "In Progress" with Maestro as assignee should be detected on startup and either resumed or moved back to "To Do" (configurable behavior: `on_restart: "resume" | "reset"`).
- Worktrees from previous runs are preserved on the mounted volume.

### 7.6 Graceful Shutdown

- On SIGTERM/SIGINT, Maestro:
  1. Stops accepting new tickets.
  2. Waits for current Claude Code sessions to complete (up to a configurable grace period).
  3. Kills remaining sessions after the grace period.
  4. Unassigns tickets and moves them back to "To Do".
  5. Exits.

---

## 8. Workflow Step Summary

For reference, the complete ordered workflow for a single ticket:

| Step | Description | Dry Mode |
|---|---|---|
| 1 | Assign ticket to Maestro user | Skip |
| 2 | Move ticket to "In Progress" | Skip |
| 3 | Retrieve ticket details + linked items | Execute |
| 4 | Create git worktree on new branch | Execute |
| 5 | Claude Code: `/address-ticket` (pass 1) | Execute |
| 6 | Claude Code: `/review-changes` (pass 1) | Execute |
| 7 | Claude Code: `/address-ticket` (pass 2) | Execute |
| 8 | Claude Code: `/review-changes` (pass 2) | Execute |
| 9 | Claude Code: `/address-ticket` (pass 3) | Execute |
| 10 | Claude Code: `/review-changes` (pass 3) | Execute |
| 11 | Run lint command, fix until clean, commit | Execute |
| 12 | Run unit tests, fix until clean, commit | Execute |
| 13 | Run e2e tests, fix until clean, commit | Execute |
| 14 | Create PR via `gh` | Skip |

On stop at any step: kill sessions, unassign (skip in dry), move to To Do (skip in dry), preserve worktree.

---

## 9. Non-Functional Requirements

- **Startup time**: Maestro should be ready to poll within 5 seconds of container start.
- **Memory**: Each workflow should use no more than 512 MB of memory (excluding Claude Code sessions).
- **Logging**: All actions are logged to stdout in structured JSON format for container log aggregation.
- **Web UI responsiveness**: Dashboard updates within 2 seconds of a workflow state change (via polling or WebSocket).
- **Security**: No secrets are logged or exposed in the web UI. Session outputs are sanitized before display.
