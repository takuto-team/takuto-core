// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Workflow response DTOs and the helpers that derive them from a `Workflow`.

use std::collections::HashMap;

use serde::Serialize;

use maestro_core::container::ContainerRunner;
use maestro_core::jira::ticket_browse_url;
use maestro_core::workflow::engine::{TerminalLine, Workflow};
use maestro_core::workflow::state::WorkflowState;
use maestro_core::workflow::step::StepLog;

#[derive(Serialize)]
pub struct TerminalLineDto {
    pub text: String,
    pub stream: String,
}

impl From<&TerminalLine> for TerminalLineDto {
    fn from(tl: &TerminalLine) -> Self {
        Self {
            text: tl.text.clone(),
            stream: tl.stream.clone(),
        }
    }
}

#[derive(Serialize)]
pub struct WorkflowSummary {
    pub id: String,
    pub ticket_key: String,
    pub ticket_summary: String,
    pub ticket_description: String,
    pub ticket_type: String,
    pub state: String,
    pub started_at: String,
    pub updated_at: String,
    pub branch_name: String,
    pub pr_url: Option<String>,
    pub pr_merged: bool,
    pub steps_log: Vec<StepLog>,
    pub error: Option<String>,
    pub terminal_lines: Vec<TerminalLineDto>,
    /// **Address PR Comments** is allowed (main flow **Done** and `pr_url` set).
    pub can_address_pr_comments: bool,
    /// **Merge base branch** is allowed (main flow **Done**, `pr_url` set, worktree exists).
    pub can_merge_base: bool,
    /// **Mark as Done** is allowed (workflow state is **Done**).
    pub can_mark_done: bool,
    /// **Delete** is allowed when the workflow is not **running** (`WorkflowState::is_active` is false),
    /// or when the workflow is on the dashboard but the driver has not been started yet.
    pub can_delete: bool,
    /// **Start** is allowed (workflow on dashboard but driver not yet spawned).
    pub can_start: bool,
    /// Step-based progress 0–100 (see `dashboard_progress` in maestro-core).
    pub progress_percent: u8,
    /// Estimated step count for the current phase (discrete progress segments / `N` in `k/N`).
    pub progress_steps_total: u32,
    /// Started via dashboard **+** manual picker.
    pub started_manually: bool,
    /// Counts against **`[general] max_concurrent_manual_workflows`** (manual start and not Done/Stopped/Error).
    pub counts_toward_manual_cap: bool,
    /// Jira **browse** URL from **`[jira] site`** + **`ticket_key`** (dashboard **Go to ticket**).
    pub jira_browse_url: String,
    /// Direct URL to the issue in the ticketing system. For GitHub workflows, this is the
    /// `html_url` stored when the workflow was created. For Jira workflows, it is the
    /// computed browse URL. `None` when no URL is available (e.g. `ticketing_system = none`).
    pub issue_url: Option<String>,
    /// **Open editor** is allowed (workflow not active, worktree exists, Docker available).
    pub can_open_editor: bool,
    /// Set when an editor container is already running for this workflow.
    pub editor_url: Option<String>,
    /// `(container_port, proxy_url)` pairs for user-configured application ports.
    pub editor_port_mappings: Vec<(u16, String)>,
    /// `true` when Jira (acli) was available when this workflow was created.
    pub jira_available: bool,
    /// Which ticketing system was active when this workflow was created: `"jira"`, `"github"`, or `"none"`.
    pub ticketing_system: String,
    /// **Resume from error** is allowed (Error or Stopped, worktree exists on disk).
    pub can_resume_from_error: bool,
    /// Set when a web terminal (ttyd) is running for this workflow's editor container.
    pub terminal_url: Option<String>,
    /// Configured run commands (from `[[run_commands]]` in config), with current running status.
    pub run_commands: Vec<RunCommandStatus>,
    /// Whether report generation is enabled in config (`[general] generate_report`).
    pub generate_report: bool,
    /// Whether a generated report file exists at `lore/reports/<key>_report.md` in the worktree.
    pub has_report: bool,
    /// Status of each dynamic workflow definition run for this ticket: def_name -> state display name.
    pub workflow_def_runs: HashMap<String, String>,
    /// Absolute path of the git worktree on disk, if it exists.
    /// `None` while the worktree is still being pre-created in the background.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<String>,
    /// ID of the user who created this workflow. `None` for legacy/poller workflows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    /// Plan-10: name of the repository (`repositories.name`) this workflow runs against.
    /// Powers the per-card repo badge on the dashboard. Always populated — every
    /// workflow has a `workspace_name` even pre-plan-10.
    pub workspace_name: String,
    /// Plan-10: FK to `repositories.id`. `None` for pre-plan-10 snapshots not yet
    /// back-filled by the startup reconciliation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository_id: Option<String>,
}

#[derive(Serialize)]
pub struct WorkflowCountsResponse {
    pub running: u32,
    pub completed: u32,
    pub errors: u32,
    pub paused: u32,
}

/// Status of a single run command.
#[derive(Serialize)]
pub struct RunCommandStatus {
    /// Index of the command in the `[[run_commands]]` config array.
    pub index: usize,
    /// Display name from config.
    pub name: String,
    /// Whether the command is currently running.
    pub running: bool,
    /// Forwarded port `(container_port, proxy_url)`, if detected.
    pub forwarded_port: Option<(u16, String)>,
}

pub(super) fn workflow_action_flags(w: &Workflow) -> (bool, bool, bool) {
    let done = matches!(w.state, WorkflowState::Done);
    let has_pr = w
        .pr_url
        .as_deref()
        .map(str::trim)
        .is_some_and(|s| !s.is_empty());
    let has_worktree = w.worktree_path.as_ref().is_some_and(|p| p.exists());
    let can_address = done && has_pr;
    let can_merge_base = done && has_pr && has_worktree;
    let can_mark = done;
    (can_address, can_merge_base, can_mark)
}

pub(super) fn manual_cap_fields(w: &Workflow) -> (bool, bool) {
    let toward = w.started_manually && w.state.occupies_concurrency_slot();
    (w.started_manually, toward)
}

pub(super) fn can_open_editor(w: &Workflow) -> bool {
    w.worktree_path.as_ref().is_some_and(|p| p.exists()) && ContainerRunner::is_available()
}

pub(super) fn has_report_file(w: &Workflow) -> bool {
    w.worktree_path.as_ref().is_some_and(|p| {
        p.join(format!("lore/reports/{}_report.md", w.ticket_key))
            .exists()
    })
}

pub(super) fn can_start_workflow(w: &Workflow) -> bool {
    matches!(w.state, WorkflowState::Pending) && !w.driver_started
}

pub(super) fn can_resume_from_error(w: &Workflow) -> bool {
    matches!(
        w.state,
        WorkflowState::Error { .. } | WorkflowState::Stopped
    ) && w.worktree_path.as_ref().is_some_and(|p| p.exists())
}

/// Compute the canonical issue URL for a workflow.
///
/// - If `ticket_url` is `Some`, use it directly (GitHub issues and future providers).
/// - If `ticket_url` is `None` and the ticketing system is Jira with a configured site,
///   compute the Jira browse URL.
/// - Otherwise `None`.
pub(super) fn build_issue_url(w: &Workflow, jira_site: &str) -> Option<String> {
    if let Some(ref url) = w.ticket_url {
        return Some(url.clone());
    }
    if w.ticketing_system == maestro_core::config::TicketingSystem::Jira && !jira_site.is_empty() {
        let url = ticket_browse_url(jira_site, &w.ticket_key);
        if !url.is_empty() {
            return Some(url);
        }
    }
    None
}

pub(super) fn workflow_def_runs_display(w: &Workflow) -> HashMap<String, String> {
    w.workflow_def_runs
        .iter()
        .map(|(k, v)| (k.clone(), v.display_name().to_string()))
        .collect()
}

pub(super) fn extract_error(state: &WorkflowState) -> Option<String> {
    match state {
        WorkflowState::Error { message, .. } => Some(message.clone()),
        _ => None,
    }
}

/// Build the run command status list for a given workflow's ticket key.
pub(super) fn build_run_commands_status(
    configured: &[maestro_core::db::user_worktree_commands::RunCommand],
    active_cmds: Option<&Vec<crate::state::ActiveRunCommand>>,
) -> Vec<RunCommandStatus> {
    configured
        .iter()
        .enumerate()
        .map(|(i, rc)| {
            let (running, forwarded_port) = if let Some(active) = active_cmds {
                if let Some(cmd_state) = active.iter().find(|c| c.cmd_index == i) {
                    (
                        true,
                        cmd_state
                            .forwarded_port
                            .as_ref()
                            .map(|f| (f.container_port, f.proxy_url.clone())),
                    )
                } else {
                    (false, None)
                }
            } else {
                (false, None)
            };
            RunCommandStatus {
                index: i,
                name: rc.name.clone(),
                running,
                forwarded_port,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal `Workflow` in `Pending` state with the given `driver_started` value.
    fn wf_pending(driver_started: bool) -> Workflow {
        let mut w = Workflow::new(
            "T-1".into(),
            "summary".into(),
            true,
            false,
            maestro_core::config::TicketingSystem::None,
            None,
            "test-workspace".into(),
        );
        w.driver_started = driver_started;
        w
    }

    #[test]
    fn can_start_pending_not_started() {
        assert!(can_start_workflow(&wf_pending(false)));
    }

    #[test]
    fn can_start_false_when_started() {
        assert!(!can_start_workflow(&wf_pending(true)));
    }

    #[test]
    fn can_start_false_when_not_pending() {
        let mut w = wf_pending(false);
        w.state = WorkflowState::Done;
        assert!(!can_start_workflow(&w));
    }
}
