//! Dashboard progress bar: completed `steps_log` entries vs an estimated total from `Config` and state.

use std::path::Path;

use crate::config::Config;
use crate::process;
use crate::workflow::engine::Workflow;
use crate::workflow::state::WorkflowState;
use crate::workflow::step::StepStatus;

/// 0–100: done workflows are always full; otherwise derived from `steps_log` and [`estimated_step_total`].
pub fn workflow_progress_percent(w: &Workflow, cfg: &Config) -> u8 {
    if matches!(w.state, WorkflowState::Done) {
        return 100;
    }

    let total = estimated_step_total(w, cfg);
    if total == 0 {
        return 0;
    }

    let mut done = w
        .steps_log
        .iter()
        .filter(|s| s.status != StepStatus::Running)
        .count() as f64;

    if in_flight_partial_credit(w) {
        done += 0.4;
    }

    let p = ((done / total as f64) * 100.0).round() as u32;
    p.min(100) as u8
}

/// Filled segment count for a discrete progress bar: rounds `progress_percent` (0–100) to the nearest step out of `total`.
pub fn workflow_progress_filled_segments(progress_percent: u8, total: u32) -> u32 {
    if total == 0 {
        return 0;
    }
    let p = progress_percent as u32;
    let filled = (p.saturating_mul(total) + 50) / 100;
    filled.min(total)
}

fn in_flight_partial_credit(w: &Workflow) -> bool {
    match &w.state {
        WorkflowState::Done
        | WorkflowState::Stopped
        | WorkflowState::Error { .. }
        | WorkflowState::Paused { .. }
        | WorkflowState::Pending => false,
        WorkflowState::Reviewing | WorkflowState::CreatingPR => true,
        WorkflowState::Assigning
        | WorkflowState::RetrievingDetails
        | WorkflowState::CreatingWorktree
        | WorkflowState::AddressingTicket { .. }
        | WorkflowState::AddressingPrComments { .. }
        | WorkflowState::MergingBaseBranch { .. } => true,
    }
}

/// Expected number of `steps_log` rows for the current phase.
///
/// When the workflow is in a **subflow** (`AddressingPrComments` / `MergingBaseBranch`),
/// return only that subflow's step count — not the main ticket total.
pub fn estimated_step_total(w: &Workflow, cfg: &Config) -> u32 {
    match &w.state {
        WorkflowState::AddressingPrComments { .. } => {
            return pr_subflow_steps(cfg).max(1);
        }
        WorkflowState::MergingBaseBranch { .. } => {
            return merge_base_subflow_steps(cfg).max(1);
        }
        _ => {}
    }

    // Main ticket pipeline
    let mut t: u32 = 3; // Assign Ticket, Retrieve Details, Create Worktree

    let path_for_mise = w
        .worktree_path
        .as_deref()
        .unwrap_or_else(|| Path::new(&cfg.git.repo_path));
    if process::worktree_has_mise_config(path_for_mise) {
        t += 1;
    }

    t += cfg.commands.pre_install.len() as u32;
    if !cfg.commands.install.trim().is_empty() {
        t += 1;
    }

    let steps = cfg.resolved_agent_steps();
    let loops = cfg.agent_sequence_outer_loops() as u32;
    let per_loop: u32 = steps.iter().map(|s| s.repeat as u32).sum();
    t += loops.saturating_mul(per_loop.max(1));

    // "Workflow complete" row after agent sequence
    t += 1;

    t.max(1)
}

fn pr_subflow_steps(cfg: &Config) -> u32 {
    let steps = cfg.resolved_review_agent_steps();
    let loops = cfg.review_sequence_outer_loops() as u32;
    let per: u32 = steps.iter().map(|s| s.repeat as u32).sum();
    // Agent steps + "PR review summary" + "PR review complete"
    loops.saturating_mul(per.max(1)) + 2
}

fn merge_base_subflow_steps(cfg: &Config) -> u32 {
    let steps = cfg.resolved_merge_base_agent_steps();
    let per: u32 = steps.iter().map(|s| s.repeat as u32).sum();
    // Agent steps + "Merge base branch complete"
    per.max(1) + 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::path::PathBuf;
    use tokio_util::sync::CancellationToken;

    use crate::workflow::step::StepLog;
    use crate::workflow::state::WorkflowState;

    fn wf_with(state: WorkflowState, steps_log: Vec<StepLog>, worktree: Option<PathBuf>) -> Workflow {
        let now = Utc::now();
        Workflow {
            id: "id".into(),
            ticket_key: "X-1".into(),
            ticket_summary: "s".into(),
            ticket_description: String::new(),
            ticket_type: "Task".into(),
            state,
            started_at: now,
            updated_at: now,
            steps_log,
            branch_name: String::new(),
            worktree_path: worktree,
            pr_url: None,
            cancel_token: CancellationToken::new(),
            terminal_lines: Vec::new(),
            current_step_label: None,
            started_manually: false,
        }
    }

    #[test]
    fn pending_with_no_logs_low_percent() {
        let w = wf_with(WorkflowState::Assigning, vec![], None);
        let cfg = Config::default();
        let p = workflow_progress_percent(&w, &cfg);
        assert!(p < 15, "expected low start, got {p}");
    }

    #[test]
    fn done_is_100() {
        let w = wf_with(WorkflowState::Done, vec![], None);
        let cfg = Config::default();
        assert_eq!(workflow_progress_percent(&w, &cfg), 100);
    }

    #[test]
    fn filled_segments_rounds_percent() {
        assert_eq!(workflow_progress_filled_segments(0, 10), 0);
        assert_eq!(workflow_progress_filled_segments(100, 10), 10);
        assert_eq!(workflow_progress_filled_segments(74, 10), 7);
        assert_eq!(workflow_progress_filled_segments(75, 10), 8);
    }
}
