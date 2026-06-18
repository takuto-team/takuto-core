// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::config::TicketingSystem;

use super::super::definitions::WorkflowDefRunState;
use super::super::snapshot::{PersistedTerminalLine, PersistedWorkflowRecord};
use super::super::state::WorkflowState;
use super::super::step::{StepLog, StepStatus};

/// Result of **Mark as Done** (Jira transition + worktree removal).
#[derive(Debug, Clone, serde::Serialize)]
pub struct MarkDoneOutcome {
    pub jira_ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jira_error: Option<String>,
    pub worktree_ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_error: Option<String>,
    pub workflow_removed: bool,
}

/// A single line of terminal output stored on the workflow for persistence
/// across page reloads. Populated by spawn_output_relay after humanizing.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TerminalLine {
    pub text: String,
    pub stream: String,
}

impl Default for WorkflowEvent {
    /// Empty-string discriminator with all-None payload fields. Used together
    /// with the `..Default::default()` struct-update syntax so callers that
    /// don't need the Phase 1 `provider_from` / `provider_to` /
    /// `affected_users` fields don't have to spell them out.
    fn default() -> Self {
        Self {
            event_type: String::new(),
            workflow_id: String::new(),
            ticket_key: String::new(),
            state: String::new(),
            timestamp: Utc::now(),
            error: None,
            step_name: None,
            output_line: None,
            stream: None,
            progress_percent: None,
            progress_steps_total: None,
            forwarded_port: None,
            pr_merged: None,
            user_id: None,
            provider_from: None,
            provider_to: None,
            affected_users: None,
            auth_warning_code: None,
            auth_warning_message: None,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WorkflowEvent {
    pub event_type: String,
    pub workflow_id: String,
    pub ticket_key: String,
    pub state: String,
    pub timestamp: chrono::DateTime<Utc>,
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_line: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<String>,
    /// Step-based dashboard progress (0–100); set on `workflow_updated` and `step_completed` when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress_percent: Option<u8>,
    /// Estimated `steps_log` row total for this phase (same basis as the segmented dashboard bar).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress_steps_total: Option<u32>,
    /// `(container_port, host_port)` for dynamic port forwarding events.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forwarded_port: Option<(u16, u16)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pr_merged: Option<bool>,
    /// Owning user_id of the workflow that produced this event. `None` for
    /// broadcast/un-scoped events that should reach every authenticated
    /// subscriber. The web layer filters per-socket so users only see events
    /// for workflows they own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    /// Phase 1 (`event_type = "provider_changed"`, 04_architecture.md §2.3):
    /// previous active provider name (e.g. `"claude"`). Other event types
    /// omit this field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_from: Option<String>,
    /// Phase 1 (`event_type = "provider_changed"`): new active provider name.
    /// Other event types omit this field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_to: Option<String>,
    /// (`event_type = "provider_changed"`): user IDs whose stored
    /// credentials need re-capture after the switch. Other event types
    /// omit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub affected_users: Option<Vec<String>>,
    /// (`event_type = "auth_warning"`): a stable error code
    /// (`"sso_authorization_required"`, `"invalid_pat"`, …) the dashboard
    /// `switch()`es on. Other event types omit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_warning_code: Option<String>,
    /// (`event_type = "auth_warning"`): human-readable message for the
    /// dashboard banner. Never contains token bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_warning_message: Option<String>,
}

#[derive(Clone)]
pub struct Workflow {
    pub id: String,
    pub ticket_key: String,
    pub ticket_summary: String,
    pub ticket_description: String,
    pub ticket_type: String,
    pub state: WorkflowState,
    pub started_at: chrono::DateTime<Utc>,
    pub updated_at: chrono::DateTime<Utc>,
    pub steps_log: Vec<StepLog>,
    pub branch_name: String,
    pub worktree_path: Option<PathBuf>,
    pub pr_url: Option<String>,
    pub pr_merged: bool,
    pub cancel_token: CancellationToken,
    /// Serialises worktree-create attempts for this workflow. Held by the
    /// background `prepare_worktree_for_ticket` task at ticket-add time and
    /// by `bootstrap_new_workflow` when the user starts a flow def, so the
    /// two cannot fight over the same on-disk path. Non-persistent — a
    /// fresh lock is created on restart (the snapshot path goes through
    /// `PersistedWorkflowRecord`, which does not carry this field).
    pub worktree_lock: Arc<tokio::sync::Mutex<()>>,
    /// Total step count for the currently-running flow definition (bootstrap
    /// estimate + agent steps + the trailing "Workflow complete" row).
    /// Set by `drive_workflow_def` before steps execute and used by
    /// `dashboard_progress::estimated_step_total`, so the progress bar's
    /// denominator stays stable across the run instead of growing every
    /// time a step gets logged. `None` until a def starts (e.g. for a
    /// Pending workflow that has never been clicked).
    pub current_def_total_steps: Option<u32>,
    /// Recent terminal output lines for persistence across page reloads.
    pub terminal_lines: Vec<TerminalLine>,
    /// Human-readable agent step label for the dashboard (e.g. `Implement (cycle 2/3, run 1/1)`).
    pub current_step_label: Option<String>,
    /// Started from the dashboard **+** picker (counts toward **`[general] max_concurrent_manual_workflows`**).
    pub started_manually: bool,
    /// `true` when Jira (acli) was available at workflow creation time.
    /// When `false`, the workflow skips all Jira operations and those steps are not counted in progress.
    pub jira_available: bool,
    /// `true` when a ticketing system (`jira` or `github`) is active for this workflow.
    /// Derived from `ticketing_system != TicketingSystem::None` at creation/restore time.
    pub ticketing_available: bool,
    /// Which ticketing system was active when this workflow was created.
    pub ticketing_system: TicketingSystem,
    /// Direct URL to the ticket in the ticketing system (e.g. GitHub issue URL).
    /// For Jira workflows this is `None` and the browse URL is computed from the site + key.
    pub ticket_url: Option<String>,
    /// Last Claude/Cursor session ID for `--resume` across container restarts.
    pub last_session_id: Option<String>,
    /// Persistent session ID shared by "Improve with AI" and "Ask AI" for this workflow,
    /// so context is maintained across multiple description-editing interactions.
    pub description_session_id: Option<String>,
    /// `true` once the workflow driver task has been spawned. `false` when added
    /// to the dashboard but not yet started by the user.
    pub driver_started: bool,
    /// Status of each dynamic workflow definition run for this ticket.
    /// Keys are workflow definition filenames (without .yml), values are run states.
    pub workflow_def_runs: HashMap<String, WorkflowDefRunState>,
    /// `true` once the full bootstrap (mise install + hooks) has completed for this workflow.
    /// When `false`, the next workflow-def start must run bootstrap even if a worktree exists
    /// (the worktree was pre-created at ticket-add time but setup has not run yet).
    pub worktree_bootstrapped: bool,
    /// `true` while the best-effort background worktree pre-creation
    /// (`prepare_worktree_for_ticket`, spawned at add-to-dashboard) is in flight.
    /// Transient runtime state — drives the dashboard's "preparing" readiness
    /// signal and is **not** persisted (a restart implies nothing is in flight).
    pub worktree_preparing: bool,
    /// Name of the workspace (repo directory name under `/workspaces/`) this workflow belongs to.
    /// Used for per-workspace snapshot isolation and dashboard filtering.
    /// Kept as a denormalised back-compat handle; `repository_id` is the
    /// durable identity.
    pub workspace_name: String,
    /// FK to `repositories.id`. Every new workflow is created against a repo
    /// the caller has added, so this is `Some` for fresh workflows. `None`
    /// covers:
    ///   * Snapshots restored from older builds (back-fill happens in
    ///     `migrate_orphan_repo_associations` reconciliation).
    ///   * Workflows whose `workspace_name` does not match any registered
    ///     `repositories` row (those stay hidden from the dashboard until
    ///     an admin re-registers).
    pub repository_id: Option<String>,
    /// ID of the user who created this workflow. `None` for poller-created workflows
    /// (pre-multi-user) or workflows restored from older snapshots.
    pub user_id: Option<String>,
    /// Credentials pinned at the workflow's first agent step. `None` for
    /// legacy workflows and for fresh workflows that haven't reached
    /// their first step yet.
    pub auth_pin: Option<crate::workflow::snapshot::AuthPin>,
}

impl Workflow {
    pub fn new(
        ticket_key: String,
        ticket_summary: String,
        started_manually: bool,
        jira_available: bool,
        ticketing_system: TicketingSystem,
        ticket_url: Option<String>,
        workspace_name: String,
    ) -> Self {
        let now = Utc::now();
        let ticketing_available = ticketing_system != TicketingSystem::None;
        Self {
            id: Uuid::new_v4().to_string(),
            ticket_key,
            ticket_summary,
            ticket_description: String::new(),
            ticket_type: "Task".to_string(),
            state: WorkflowState::Pending,
            started_at: now,
            updated_at: now,
            steps_log: Vec::new(),
            branch_name: String::new(),
            worktree_path: None,
            pr_url: None,
            pr_merged: false,
            cancel_token: CancellationToken::new(),
            worktree_lock: Arc::new(tokio::sync::Mutex::new(())),
            current_def_total_steps: None,
            terminal_lines: Vec::new(),
            current_step_label: None,
            started_manually,
            jira_available,
            ticketing_available,
            ticketing_system,
            ticket_url,
            last_session_id: None,
            description_session_id: None,
            driver_started: false,
            workflow_def_runs: HashMap::new(),
            worktree_bootstrapped: false,
            worktree_preparing: false,
            workspace_name,
            repository_id: None,
            user_id: None,
            auth_pin: None,
        }
    }

    /// Convert this in-memory Workflow into the row shape the work_items
    /// table stores. The map is still the truth-of-record; this
    /// conversion is used by the shadow-write pass in `start_workflow` /
    /// `add_to_dashboard` so the DB has a faithful row alongside every
    /// in-memory entry.
    ///
    /// Lossy by design for fields that don't have a column yet
    /// (`steps_log`, `terminal_lines`, `workflow_def_runs`,
    /// `worktree_bootstrapped`, `description_session_id`, `auth_pin`,
    /// `repository_id`, `ticketing_system`, `ticketing_available`,
    /// `cancel_token`). Those will land in follow-up slices when the
    /// child tables get wired in.
    pub fn to_work_item_row(&self) -> crate::db::work_items::WorkItemRow {
        use crate::db::work_items::WorkItemRow;
        let (state_kind, state_payload) = state_to_kind_and_payload(&self.state);
        WorkItemRow {
            id: self.id.clone(),
            ticket_key: self.ticket_key.clone(),
            workspace_name: self.workspace_name.clone(),
            user_id: self.user_id.clone(),
            // Shadow-write the repo association so the DB row carries
            // everything `require_workflow_access` consults.
            repository_id: self.repository_id.clone(),
            // `private` belongs to the visibility model; default to public
            // on every fresh insert until that flag lands on Workflow.
            private: false,
            started_manually: self.started_manually,
            // Manual cap accounting is engine-internal today; pin to
            // started_manually as a placeholder until the engine
            // surfaces the actual bit.
            counts_toward_manual_cap: self.started_manually,
            driver_started: self.driver_started,
            jira_available: self.jira_available,
            ticket_summary: Some(self.ticket_summary.clone()).filter(|s| !s.is_empty()),
            ticket_description: Some(self.ticket_description.clone()).filter(|s| !s.is_empty()),
            ticket_type: Some(self.ticket_type.clone()).filter(|s| !s.is_empty()),
            ticket_url: self.ticket_url.clone(),
            // Not surfaced on `Workflow` today.
            acceptance_criteria: None,
            // Not surfaced on `Workflow` today.
            base_branch: None,
            branch_name: Some(self.branch_name.clone()).filter(|s| !s.is_empty()),
            worktree_path: self.worktree_path.as_ref().map(|p| p.display().to_string()),
            pr_url: self.pr_url.clone(),
            pr_merged: self.pr_merged,
            last_session_id: self.last_session_id.clone(),
            state_kind,
            state_payload,
            current_step_label: self.current_step_label.clone(),
            created_at: self.started_at.timestamp(),
            started_at: self.started_at.timestamp(),
            updated_at: self.updated_at.timestamp(),
        }
    }

    /// String shown on the dashboard and in WebSocket `workflow_updated` events.
    pub fn status_display(&self) -> String {
        match &self.state {
            WorkflowState::Paused { .. }
            | WorkflowState::Error { .. }
            | WorkflowState::Done
            | WorkflowState::Stopped => self.state.display_name(),
            WorkflowState::AddressingTicket { .. } => self
                .current_step_label
                .clone()
                .unwrap_or_else(|| "Running agent steps".to_string()),
            WorkflowState::AddressingPrComments { .. } => self
                .current_step_label
                .clone()
                .unwrap_or_else(|| "Addressing PR comments".to_string()),
            _ => self.state.display_name(),
        }
    }

    pub(crate) fn from_persisted_record(rec: PersistedWorkflowRecord) -> Self {
        // Drop "Running" entries from the snapshot — they represent steps that were interrupted
        // by a graceful shutdown and will be re-executed on resume. Keeping them would create
        // duplicate entries (old Running + new Success/Failed) and inflate the progress count.
        let steps_log: Vec<StepLog> = rec
            .steps_log
            .into_iter()
            .filter(|s| s.status != StepStatus::Running)
            .collect();
        // Backward compatibility: old snapshots lack `ticketing_system` (deserializes as `None`)
        // but may have `jira_available = true`. Derive the correct ticketing system so that
        // `when: ticketing` steps are not incorrectly filtered out on resume.
        let ticketing_system =
            if rec.ticketing_system == TicketingSystem::None && rec.jira_available {
                TicketingSystem::Jira
            } else {
                rec.ticketing_system
            };
        let ticketing_available = ticketing_system != TicketingSystem::None;
        Self {
            id: rec.id,
            ticket_key: rec.ticket_key,
            ticket_summary: rec.ticket_summary,
            ticket_description: rec.ticket_description,
            ticket_type: rec.ticket_type,
            state: rec.state,
            started_at: rec.started_at,
            updated_at: rec.updated_at,
            steps_log,
            branch_name: rec.branch_name,
            worktree_path: rec.worktree_path,
            pr_url: rec.pr_url,
            pr_merged: rec.pr_merged,
            cancel_token: CancellationToken::new(),
            worktree_lock: Arc::new(tokio::sync::Mutex::new(())),
            current_def_total_steps: None,
            terminal_lines: rec
                .terminal_lines
                .into_iter()
                .map(|l| TerminalLine {
                    text: l.text,
                    stream: l.stream,
                })
                .collect(),
            current_step_label: rec.current_step_label,
            started_manually: rec.started_manually,
            jira_available: rec.jira_available,
            ticketing_available,
            ticketing_system,
            ticket_url: rec.ticket_url,
            last_session_id: rec.last_session_id,
            description_session_id: rec.description_session_id,
            driver_started: rec.driver_started,
            workflow_def_runs: rec.workflow_def_runs,
            worktree_bootstrapped: rec.worktree_bootstrapped,
            // Transient — nothing is in flight immediately after a restore.
            worktree_preparing: false,
            workspace_name: rec.workspace_name,
            repository_id: rec.repository_id,
            user_id: rec.user_id,
            auth_pin: rec.auth_pin,
        }
    }

    /// Reconstruct a live `Workflow` from its authoritative `work_items` row
    /// plus its `work_item_definition_runs` rows — the inverse of
    /// [`Workflow::to_work_item_row`], used by the DB-first restore path
    /// (cutover invariant I3).
    ///
    /// Fields the DB does not store get safe defaults here and are
    /// supplemented from the snapshot (when one exists) by the restore caller:
    /// `terminal_lines`, `description_session_id`, `auth_pin`,
    /// `worktree_bootstrapped`, and `ticketing_system` (which the DB only
    /// approximates via `jira_available`). `steps_log` is left empty — the
    /// read path serves steps from `work_item_steps`, and the driver-respawn
    /// pass keys off `workflow_def_runs`, not the in-memory log. Runtime
    /// handles (`cancel_token`, `worktree_lock`) are created fresh.
    ///
    /// `worktree_bootstrapped` deliberately defaults to `false`: `worktree_path`
    /// is persisted mid-bootstrap (before init commands run), so a present
    /// path does NOT prove bootstrap finished. Re-bootstrap is idempotent
    /// (`create_worktree` clears leftovers), so erring toward re-running is
    /// safe; a half-bootstrapped row must not skip its init commands.
    pub(crate) fn from_work_item_row(
        row: crate::db::work_items::WorkItemRow,
        def_runs: Vec<crate::db::work_items::DefinitionRunRow>,
    ) -> Self {
        use crate::db::work_items::DefRunState;

        let state = state_from_kind_and_payload(row.state_kind, row.state_payload.as_deref());
        let started_at =
            chrono::DateTime::from_timestamp(row.started_at, 0).unwrap_or_else(Utc::now);
        let updated_at =
            chrono::DateTime::from_timestamp(row.updated_at, 0).unwrap_or_else(Utc::now);
        // The DB has no dedicated ticketing_system column; approximate from
        // `jira_available` (Jira ⇒ Jira, else None). The restore caller
        // upgrades this from the snapshot for github-mode rows when present.
        let ticketing_system = if row.jira_available {
            TicketingSystem::Jira
        } else {
            TicketingSystem::None
        };
        let ticketing_available = ticketing_system != TicketingSystem::None;

        let workflow_def_runs = def_runs
            .into_iter()
            .map(|d| {
                let state = match d.state {
                    DefRunState::Idle => WorkflowDefRunState::Idle,
                    DefRunState::Running => WorkflowDefRunState::Running,
                    DefRunState::Completed => WorkflowDefRunState::Completed,
                    DefRunState::Error => WorkflowDefRunState::Error {
                        message: d.error_message.unwrap_or_default(),
                    },
                };
                (d.definition_filename, state)
            })
            .collect();

        Self {
            id: row.id,
            ticket_key: row.ticket_key,
            ticket_summary: row.ticket_summary.unwrap_or_default(),
            ticket_description: row.ticket_description.unwrap_or_default(),
            ticket_type: row.ticket_type.unwrap_or_default(),
            state,
            started_at,
            updated_at,
            steps_log: Vec::new(),
            branch_name: row.branch_name.unwrap_or_default(),
            worktree_path: row.worktree_path.map(PathBuf::from),
            pr_url: row.pr_url,
            pr_merged: row.pr_merged,
            cancel_token: CancellationToken::new(),
            worktree_lock: Arc::new(tokio::sync::Mutex::new(())),
            current_def_total_steps: None,
            terminal_lines: Vec::new(),
            current_step_label: row.current_step_label,
            started_manually: row.started_manually,
            jira_available: row.jira_available,
            ticketing_available,
            ticketing_system,
            ticket_url: row.ticket_url,
            last_session_id: row.last_session_id,
            description_session_id: None,
            driver_started: row.driver_started,
            workflow_def_runs,
            worktree_bootstrapped: false,
            worktree_preparing: false,
            workspace_name: row.workspace_name,
            repository_id: row.repository_id,
            user_id: row.user_id,
            auth_pin: None,
        }
    }
}

pub(super) fn workflow_to_persisted_record(w: &Workflow) -> PersistedWorkflowRecord {
    PersistedWorkflowRecord {
        id: w.id.clone(),
        ticket_key: w.ticket_key.clone(),
        ticket_summary: w.ticket_summary.clone(),
        ticket_description: w.ticket_description.clone(),
        ticket_type: w.ticket_type.clone(),
        state: w.state.clone(),
        started_at: w.started_at,
        updated_at: w.updated_at,
        steps_log: w.steps_log.clone(),
        branch_name: w.branch_name.clone(),
        worktree_path: w.worktree_path.clone(),
        pr_url: w.pr_url.clone(),
        pr_merged: w.pr_merged,
        terminal_lines: w
            .terminal_lines
            .iter()
            .map(|l| PersistedTerminalLine {
                text: l.text.clone(),
                stream: l.stream.clone(),
            })
            .collect(),
        current_step_label: w.current_step_label.clone(),
        started_manually: w.started_manually,
        jira_available: w.jira_available,
        last_session_id: w.last_session_id.clone(),
        description_session_id: w.description_session_id.clone(),
        ticketing_system: w.ticketing_system,
        ticket_url: w.ticket_url.clone(),
        driver_started: w.driver_started,
        workflow_def_runs: w.workflow_def_runs.clone(),
        worktree_bootstrapped: w.worktree_bootstrapped,
        workspace_name: w.workspace_name.clone(),
        repository_id: w.repository_id.clone(),
        user_id: w.user_id.clone(),
        auth_pin: w.auth_pin.clone(),
    }
}

/// Split a `WorkflowState` into the `(state_kind, state_payload)` pair
/// stored on `work_items`.
///
/// The payload is a JSON object carrying any variant data (e.g.
/// `{"pass": 2}` for `AddressingTicket { pass: 2 }`, or
/// `{"source_state": {...}, "message": "..."}` for `Error`). When the
/// variant carries no data, payload is `None`. The full state can be
/// reconstructed by combining the kind discriminator with the payload
/// — the engine's existing `serde_json::to_string(&self.state)` round-
/// trips losslessly.
pub(crate) fn state_to_kind_and_payload(
    state: &crate::workflow::state::WorkflowState,
) -> (crate::db::work_items::WorkItemStateKind, Option<String>) {
    use crate::db::work_items::WorkItemStateKind;
    use crate::workflow::state::WorkflowState;
    let kind = match state {
        WorkflowState::Pending => WorkItemStateKind::Pending,
        WorkflowState::Assigning => WorkItemStateKind::Assigning,
        WorkflowState::RetrievingDetails => WorkItemStateKind::RetrievingDetails,
        WorkflowState::CreatingWorktree => WorkItemStateKind::CreatingWorktree,
        WorkflowState::AddressingTicket { .. } => WorkItemStateKind::AddressingTicket,
        WorkflowState::AddressingPrComments { .. } => WorkItemStateKind::AddressingPrComments,
        WorkflowState::MergingBaseBranch { .. } => WorkItemStateKind::MergingBaseBranch,
        WorkflowState::Reviewing => WorkItemStateKind::Reviewing,
        WorkflowState::CreatingPR => WorkItemStateKind::CreatingPr,
        WorkflowState::Done => WorkItemStateKind::Done,
        WorkflowState::Stopped => WorkItemStateKind::Stopped,
        WorkflowState::Error { .. } => WorkItemStateKind::Error,
        WorkflowState::Paused { .. } => WorkItemStateKind::Paused,
    };
    // Only serialise the payload for variants that carry data —
    // skipping unit-like variants keeps the column NULL on the common
    // path and avoids storing `"\"Pending\""` for every fresh row.
    let payload = match state {
        WorkflowState::Pending
        | WorkflowState::Assigning
        | WorkflowState::RetrievingDetails
        | WorkflowState::CreatingWorktree
        | WorkflowState::Reviewing
        | WorkflowState::CreatingPR
        | WorkflowState::Done
        | WorkflowState::Stopped => None,
        _ => serde_json::to_string(state).ok(),
    };
    (kind, payload)
}

/// Inverse of [`state_to_kind_and_payload`]: reconstruct a [`WorkflowState`]
/// from its persisted `state_kind` + `state_payload`.
///
/// Data-carrying variants (`AddressingTicket`, `Error`, `Paused`, and the two
/// legacy variants) were stored as the full serde-JSON of the state in the
/// payload column, so they round-trip via `serde_json::from_str`. Unit
/// variants carry no payload and map directly from the kind. A data-carrying
/// kind whose payload is missing or undecodable lands in `Error` (rather than
/// panicking) so the divergence is visible on the dashboard.
pub(crate) fn state_from_kind_and_payload(
    kind: crate::db::work_items::WorkItemStateKind,
    payload: Option<&str>,
) -> WorkflowState {
    use crate::db::work_items::WorkItemStateKind as K;
    match kind {
        K::Pending => WorkflowState::Pending,
        K::Assigning => WorkflowState::Assigning,
        K::RetrievingDetails => WorkflowState::RetrievingDetails,
        K::CreatingWorktree => WorkflowState::CreatingWorktree,
        K::Reviewing => WorkflowState::Reviewing,
        K::CreatingPr => WorkflowState::CreatingPR,
        K::Done => WorkflowState::Done,
        K::Stopped => WorkflowState::Stopped,
        K::AddressingTicket
        | K::AddressingPrComments
        | K::MergingBaseBranch
        | K::Error
        | K::Paused => payload
            .and_then(|p| serde_json::from_str::<WorkflowState>(p).ok())
            .unwrap_or_else(|| WorkflowState::Error {
                source_state: Box::new(WorkflowState::Pending),
                message: format!(
                    "restored work item had state_kind={} with no decodable state payload",
                    kind.as_str()
                ),
            }),
    }
}

#[cfg(test)]
mod facade_workflow_tests {
    use super::*;
    use crate::config::TicketingSystem;
    use crate::workflow::definitions::WorkflowDefRunState;
    use crate::workflow::snapshot::{PersistedTerminalLine, PersistedWorkflowRecord};
    use crate::workflow::state::WorkflowState;
    use crate::workflow::step::{StepLog, StepStatus};
    use chrono::Utc;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use tokio_util::sync::CancellationToken;

    /// Helper to build a `Workflow` for testing, following the `dashboard_progress.rs` pattern.
    fn wf_with(
        state: WorkflowState,
        steps_log: Vec<StepLog>,
        current_step_label: Option<String>,
    ) -> Workflow {
        let now = Utc::now();
        Workflow {
            id: "test-id".into(),
            ticket_key: "TEST-1".into(),
            ticket_summary: "Test summary".into(),
            ticket_description: String::new(),
            ticket_type: "Task".into(),
            state,
            started_at: now,
            updated_at: now,
            steps_log,
            branch_name: String::new(),
            worktree_path: None,
            pr_url: None,
            pr_merged: false,
            cancel_token: CancellationToken::new(),
            worktree_lock: std::sync::Arc::new(tokio::sync::Mutex::new(())),
            current_def_total_steps: None,
            terminal_lines: Vec::new(),
            current_step_label,
            started_manually: false,
            jira_available: true,
            ticketing_available: true,
            ticketing_system: TicketingSystem::Jira,
            ticket_url: None,
            last_session_id: None,
            description_session_id: None,
            driver_started: true,
            workflow_def_runs: HashMap::new(),
            worktree_bootstrapped: false,
            worktree_preparing: false,
            workspace_name: "test-workspace".into(),
            repository_id: None,
            user_id: None,
            auth_pin: None,
        }
    }
    // -----------------------------------------------------------------------
    // 5. Workflow::status_display() — delegates to current_step_label for
    //    AddressingTicket/AddressingPrComments, falls back to display_name for others
    // -----------------------------------------------------------------------

    #[test]
    fn status_display_addressing_ticket_uses_step_label() {
        let w = wf_with(
            WorkflowState::AddressingTicket { pass: 1 },
            vec![],
            Some("Implement ticket (cycle 1/3, run 1/1)".into()),
        );
        assert_eq!(w.status_display(), "Implement ticket (cycle 1/3, run 1/1)");
    }

    #[test]
    fn status_display_addressing_ticket_fallback_no_label() {
        let w = wf_with(WorkflowState::AddressingTicket { pass: 1 }, vec![], None);
        assert_eq!(w.status_display(), "Running agent steps");
    }

    #[test]
    fn status_display_addressing_pr_comments_uses_step_label() {
        let w = wf_with(
            WorkflowState::AddressingPrComments { pass: 1 },
            vec![],
            Some("Review PR (cycle 2/3, run 1/1)".into()),
        );
        assert_eq!(w.status_display(), "Review PR (cycle 2/3, run 1/1)");
    }

    #[test]
    fn status_display_addressing_pr_comments_fallback_no_label() {
        let w = wf_with(
            WorkflowState::AddressingPrComments { pass: 1 },
            vec![],
            None,
        );
        assert_eq!(w.status_display(), "Addressing PR comments");
    }

    #[test]
    fn status_display_done_delegates_to_display_name() {
        let w = wf_with(WorkflowState::Done, vec![], None);
        assert_eq!(w.status_display(), "Done");
    }

    #[test]
    fn status_display_paused_delegates_to_display_name() {
        let w = wf_with(
            WorkflowState::Paused {
                source_state: Box::new(WorkflowState::Assigning),
            },
            vec![],
            Some("should be ignored".into()),
        );
        assert_eq!(w.status_display(), "Paused");
    }

    #[test]
    fn status_display_error_delegates_to_display_name() {
        let w = wf_with(
            WorkflowState::Error {
                source_state: Box::new(WorkflowState::Pending),
                message: "oops".into(),
            },
            vec![],
            None,
        );
        assert_eq!(w.status_display(), "Error: oops");
    }

    #[test]
    fn status_display_stopped_delegates_to_display_name() {
        let w = wf_with(WorkflowState::Stopped, vec![], None);
        assert_eq!(w.status_display(), "Stopped");
    }

    #[test]
    fn status_display_assigning_delegates_to_display_name() {
        let w = wf_with(WorkflowState::Assigning, vec![], None);
        assert_eq!(w.status_display(), "Assigning Ticket");
    }

    #[test]
    fn status_display_creating_worktree_delegates_to_display_name() {
        let w = wf_with(WorkflowState::CreatingWorktree, vec![], None);
        assert_eq!(w.status_display(), "Creating Worktree");
    }

    // -----------------------------------------------------------------------
    // 6. Workflow::from_persisted_record()
    // -----------------------------------------------------------------------

    /// Build a minimal `PersistedWorkflowRecord` for testing.
    fn make_persisted_record(
        state: WorkflowState,
        driver_started: bool,
    ) -> PersistedWorkflowRecord {
        let now = Utc::now();
        PersistedWorkflowRecord {
            id: "rec-id".into(),
            ticket_key: "REC-1".into(),
            ticket_summary: "Record summary".into(),
            ticket_description: "desc".into(),
            ticket_type: "Bug".into(),
            state,
            started_at: now,
            updated_at: now,
            steps_log: vec![],
            branch_name: "feat/rec-1".into(),
            worktree_path: Some(PathBuf::from("/tmp/wt")),
            pr_url: Some("https://github.com/foo/bar/pull/42".into()),
            pr_merged: true,
            terminal_lines: vec![PersistedTerminalLine {
                text: "hello".into(),
                stream: "stdout".into(),
            }],
            current_step_label: Some("Step label".into()),
            started_manually: true,
            jira_available: false,
            last_session_id: Some("sess-abc".into()),
            description_session_id: Some("desc-xyz".into()),
            ticketing_system: TicketingSystem::GitHub,
            ticket_url: Some("https://github.com/foo/bar/issues/1".into()),
            driver_started,
            workflow_def_runs: {
                let mut m = HashMap::new();
                m.insert("implement_ticket".into(), WorkflowDefRunState::Completed);
                m
            },
            worktree_bootstrapped: true,
            workspace_name: "test-workspace".into(),
            repository_id: None,
            user_id: None,
            auth_pin: None,
        }
    }

    #[test]
    fn from_persisted_record_round_trips_modern_state() {
        let rec = make_persisted_record(WorkflowState::AddressingTicket { pass: 1 }, true);
        let w = Workflow::from_persisted_record(rec);
        assert_eq!(w.id, "rec-id");
        assert_eq!(w.ticket_key, "REC-1");
        assert_eq!(w.ticket_summary, "Record summary");
        assert_eq!(w.ticket_description, "desc");
        assert_eq!(w.ticket_type, "Bug");
        assert!(matches!(
            w.state,
            WorkflowState::AddressingTicket { pass: 1 }
        ));
        assert_eq!(w.branch_name, "feat/rec-1");
        assert_eq!(w.worktree_path, Some(PathBuf::from("/tmp/wt")));
        assert_eq!(
            w.pr_url.as_deref(),
            Some("https://github.com/foo/bar/pull/42")
        );
        assert!(w.pr_merged);
        assert_eq!(w.terminal_lines.len(), 1);
        assert_eq!(w.terminal_lines[0].text, "hello");
        assert_eq!(w.terminal_lines[0].stream, "stdout");
        assert_eq!(w.current_step_label.as_deref(), Some("Step label"));
        assert!(w.started_manually);
        assert!(!w.jira_available);
        assert_eq!(w.last_session_id.as_deref(), Some("sess-abc"));
        assert_eq!(w.description_session_id.as_deref(), Some("desc-xyz"));
        assert_eq!(w.ticketing_system, TicketingSystem::GitHub);
        assert!(w.ticketing_available);
        assert!(w.driver_started);
        assert_eq!(
            w.workflow_def_runs.get("implement_ticket"),
            Some(&WorkflowDefRunState::Completed)
        );
        assert!(w.worktree_bootstrapped);
    }

    #[test]
    fn from_persisted_record_strips_running_steps() {
        let now = Utc::now();
        let rec = PersistedWorkflowRecord {
            steps_log: vec![
                StepLog {
                    step_name: "done step".into(),
                    started_at: now,
                    completed_at: Some(now),
                    status: StepStatus::Success,
                    output: vec![],
                    error: None,
                    bootstrap: false,
                },
                StepLog {
                    step_name: "running step".into(),
                    started_at: now,
                    completed_at: None,
                    status: StepStatus::Running,
                    output: vec![],
                    error: None,
                    bootstrap: false,
                },
            ],
            ..make_persisted_record(WorkflowState::AddressingTicket { pass: 1 }, true)
        };
        let w = Workflow::from_persisted_record(rec);
        assert_eq!(w.steps_log.len(), 1, "Running steps should be stripped");
        assert_eq!(w.steps_log[0].step_name, "done step");
    }

    #[test]
    fn from_persisted_record_legacy_states_deserialize() {
        // Legacy states should not panic when loaded from snapshot
        for json_state in [
            r#"{"AddressingPrComments":{"pass":1}}"#,
            r#"{"MergingBaseBranch":{"pass":2}}"#,
            r#""Reviewing""#,
            r#""CreatingPR""#,
        ] {
            let state: WorkflowState = serde_json::from_str(json_state)
                .unwrap_or_else(|e| panic!("Failed to deserialize {json_state}: {e}"));
            let rec = make_persisted_record(state, true);
            let w = Workflow::from_persisted_record(rec);
            // Just verify it doesn't panic and produces a valid workflow
            let _ = w.status_display();
        }
    }

    #[test]
    fn from_persisted_record_pending_driver_not_started() {
        let rec = make_persisted_record(WorkflowState::Pending, false);
        let w = Workflow::from_persisted_record(rec);
        assert!(matches!(w.state, WorkflowState::Pending));
        assert!(!w.driver_started);
    }

    #[test]
    fn from_persisted_record_backward_compat_ticketing_system() {
        // Old snapshots have ticketing_system = None but jira_available = true.
        // from_persisted_record should derive TicketingSystem::Jira.
        let now = Utc::now();
        let rec = PersistedWorkflowRecord {
            id: "old-id".into(),
            ticket_key: "OLD-1".into(),
            ticket_summary: "s".into(),
            ticket_description: String::new(),
            ticket_type: "Task".into(),
            state: WorkflowState::Done,
            started_at: now,
            updated_at: now,
            steps_log: vec![],
            branch_name: String::new(),
            worktree_path: None,
            pr_url: None,
            pr_merged: false,
            terminal_lines: vec![],
            current_step_label: None,
            started_manually: false,
            jira_available: true,
            last_session_id: None,
            description_session_id: None,
            ticketing_system: TicketingSystem::None,
            ticket_url: None,
            driver_started: true,
            workflow_def_runs: HashMap::new(),
            worktree_bootstrapped: false,
            workspace_name: String::new(),
            repository_id: None,
            user_id: None,
            auth_pin: None,
        };
        let w = Workflow::from_persisted_record(rec);
        assert_eq!(
            w.ticketing_system,
            TicketingSystem::Jira,
            "Old snapshot with jira_available=true should derive Jira ticketing"
        );
        assert!(w.ticketing_available);
    }

    // -----------------------------------------------------------------------
    // 7. Snapshot serde round-trip
    // -----------------------------------------------------------------------

    #[test]
    fn snapshot_record_serde_round_trip() {
        let mut def_runs = HashMap::new();
        def_runs.insert("implement_ticket".into(), WorkflowDefRunState::Completed);
        def_runs.insert("address_pr_comments".into(), WorkflowDefRunState::Idle);

        let now = Utc::now();
        let rec = PersistedWorkflowRecord {
            id: "uuid-1".into(),
            ticket_key: "SNAP-1".into(),
            ticket_summary: "Snapshot test".into(),
            ticket_description: "description".into(),
            ticket_type: "Story".into(),
            state: WorkflowState::AddressingTicket { pass: 1 },
            started_at: now,
            updated_at: now,
            steps_log: vec![StepLog {
                step_name: "Implement".into(),
                started_at: now,
                completed_at: Some(now),
                status: StepStatus::Success,
                output: vec!["line1".into()],
                error: None,
                bootstrap: false,
            }],
            branch_name: "feat/snap-1".into(),
            worktree_path: Some(PathBuf::from("/workspace/worktrees/snap-1")),
            pr_url: Some("https://github.com/org/repo/pull/99".into()),
            pr_merged: true,
            terminal_lines: vec![
                PersistedTerminalLine {
                    text: "stdout line".into(),
                    stream: "stdout".into(),
                },
                PersistedTerminalLine {
                    text: "stderr line".into(),
                    stream: "stderr".into(),
                },
            ],
            current_step_label: Some("Implement (cycle 1/2, run 1/1)".into()),
            started_manually: true,
            jira_available: false,
            last_session_id: Some("session-123".into()),
            description_session_id: Some("desc-456".into()),
            ticketing_system: TicketingSystem::GitHub,
            ticket_url: Some("https://github.com/org/repo/issues/42".into()),
            driver_started: true,
            workflow_def_runs: def_runs,
            worktree_bootstrapped: true,
            workspace_name: "test-workspace".into(),
            repository_id: Some("repo-uuid-99".into()),
            user_id: Some("user-abc".into()),
            auth_pin: None,
        };

        let json = serde_json::to_string_pretty(&rec).expect("serialize PersistedWorkflowRecord");
        let back: PersistedWorkflowRecord =
            serde_json::from_str(&json).expect("deserialize PersistedWorkflowRecord");

        assert_eq!(back.id, "uuid-1");
        assert_eq!(back.ticket_key, "SNAP-1");
        assert_eq!(back.ticket_summary, "Snapshot test");
        assert_eq!(back.ticket_description, "description");
        assert_eq!(back.ticket_type, "Story");
        assert!(matches!(
            back.state,
            WorkflowState::AddressingTicket { pass: 1 }
        ));
        assert_eq!(back.branch_name, "feat/snap-1");
        assert_eq!(
            back.worktree_path,
            Some(PathBuf::from("/workspace/worktrees/snap-1"))
        );
        assert_eq!(
            back.pr_url.as_deref(),
            Some("https://github.com/org/repo/pull/99")
        );
        assert!(back.pr_merged);
        assert_eq!(back.terminal_lines.len(), 2);
        assert_eq!(back.terminal_lines[0].text, "stdout line");
        assert_eq!(back.terminal_lines[1].stream, "stderr");
        assert_eq!(
            back.current_step_label.as_deref(),
            Some("Implement (cycle 1/2, run 1/1)")
        );
        assert!(back.started_manually);
        assert!(!back.jira_available);
        assert_eq!(back.last_session_id.as_deref(), Some("session-123"));
        assert_eq!(back.description_session_id.as_deref(), Some("desc-456"));
        assert_eq!(back.ticketing_system, TicketingSystem::GitHub);
        assert!(back.driver_started);
        assert_eq!(
            back.workflow_def_runs.get("implement_ticket"),
            Some(&WorkflowDefRunState::Completed)
        );
        assert_eq!(
            back.workflow_def_runs.get("address_pr_comments"),
            Some(&WorkflowDefRunState::Idle)
        );
        assert!(back.worktree_bootstrapped);
        assert_eq!(back.steps_log.len(), 1);
        assert_eq!(back.steps_log[0].step_name, "Implement");
        assert_eq!(back.steps_log[0].status, StepStatus::Success);
    }

    #[test]
    fn snapshot_record_defaults_for_missing_fields() {
        // Simulate an old snapshot missing newer fields
        let json = r#"{
            "id": "old",
            "ticket_key": "OLD-1",
            "ticket_summary": "s",
            "ticket_description": "",
            "ticket_type": "Task",
            "state": "Done",
            "started_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
            "steps_log": [],
            "branch_name": "",
            "worktree_path": null,
            "pr_url": null,
            "pr_merged": false,
            "terminal_lines": [],
            "current_step_label": null,
            "started_manually": false,
            "jira_available": true
        }"#;
        let rec: PersistedWorkflowRecord = serde_json::from_str(json)
            .expect("old snapshot without newer fields should deserialize");
        assert!(rec.driver_started, "driver_started should default to true");
        assert!(rec.workflow_def_runs.is_empty());
        assert!(!rec.pr_merged);
        assert!(rec.last_session_id.is_none());
        assert!(rec.description_session_id.is_none());
        assert_eq!(rec.ticketing_system, TicketingSystem::None);
        assert!(!rec.worktree_bootstrapped);
    }
}
