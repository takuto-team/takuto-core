// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::collections::HashMap;
use std::path::PathBuf;

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
}

impl Workflow {
    pub fn new(
        ticket_key: String,
        ticket_summary: String,
        started_manually: bool,
        jira_available: bool,
        ticketing_system: TicketingSystem,
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
            terminal_lines: Vec::new(),
            current_step_label: None,
            started_manually,
            jira_available,
            ticketing_available,
            ticketing_system,
            last_session_id: None,
            description_session_id: None,
            driver_started: false,
            workflow_def_runs: HashMap::new(),
            worktree_bootstrapped: false,
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
            last_session_id: rec.last_session_id,
            description_session_id: rec.description_session_id,
            driver_started: rec.driver_started,
            workflow_def_runs: rec.workflow_def_runs,
            worktree_bootstrapped: rec.worktree_bootstrapped,
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
        driver_started: w.driver_started,
        workflow_def_runs: w.workflow_def_runs.clone(),
        worktree_bootstrapped: w.worktree_bootstrapped,
    }
}
