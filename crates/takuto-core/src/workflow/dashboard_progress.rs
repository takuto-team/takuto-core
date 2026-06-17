// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

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

/// Dashboard progress fields `(percent, total)` for a workflow.
///
/// A **completed** workflow renders the authoritative persisted step count of
/// its latest completed flow at 100%, when that count is known. This matters
/// after a restart: `current_def_total_steps` is not persisted and the
/// DB-restored `steps_log` is empty, so [`estimated_step_total`] would
/// otherwise fall back to a heuristic that floors around 3 (the "3/3" bug).
///
/// Every other case — in-progress workflows, and terminals without a persisted
/// count (e.g. a run that errored before any flow completed) — falls back to
/// the in-memory estimate, preserving existing behaviour. The override is
/// scoped to `Done` so a failed/stopped workflow is never shown as 100%.
pub fn progress_fields(
    w: &Workflow,
    cfg: &Config,
    persisted_completed_steps: Option<u32>,
) -> (u8, u32) {
    if matches!(w.state, WorkflowState::Done)
        && let Some(n) = persisted_completed_steps
        && n > 0
    {
        return (100, n);
    }
    (
        workflow_progress_percent(w, cfg),
        estimated_step_total(w, cfg),
    )
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

/// Expected number of `steps_log` rows for the current workflow.
///
/// Since agent steps now come from dynamic YAML workflow definitions (not static config),
/// we estimate the total from bootstrap steps + the number of already-logged steps +
/// a small buffer for remaining work.
pub fn estimated_step_total(w: &Workflow, cfg: &Config) -> u32 {
    // When the driver has cached the total for the running def, trust it —
    // the heuristic below derives the total from `steps_log.len()` and
    // therefore drifts upward as the run progresses, which is exactly what
    // the dashboard's "k/N" denominator should not do.
    if let Some(t) = w.current_def_total_steps {
        return t.max(1);
    }

    // Bootstrap steps:
    // Assign + Retrieve only run when Jira (acli) is available — GitHub Issues skips them.
    // Jira: 3 steps (Assign + Retrieve + Worktree). GitHub/none: 1 step (Worktree only).
    let mut t: u32 = if w.jira_available { 3 } else { 1 };

    let path_for_mise = w
        .worktree_path
        .as_deref()
        .unwrap_or_else(|| Path::new(&cfg.git.repo_path));
    if process::worktree_has_mise_config(path_for_mise) {
        t += 1;
    }

    // Worktree init commands are now per-user-per-workspace; they are not
    // factored into the bootstrap estimate any more. The agent step
    // estimate below will cover them via the actual `steps_log` count.

    // For agent steps, use the current steps_log count as a lower bound.
    // If the workflow is still in progress, add a small buffer so the progress bar
    // doesn't hit 100% prematurely.
    let logged = w.steps_log.len() as u32;
    let in_progress = !w.state.is_terminal();
    let agent_estimate = if in_progress {
        logged.saturating_sub(t) + 2 // at least 2 more steps expected
    } else {
        logged.saturating_sub(t)
    };
    t += agent_estimate.max(1);

    // "Workflow complete" row after agent sequence
    t += 1;

    t.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::path::PathBuf;
    use tokio_util::sync::CancellationToken;

    use crate::workflow::state::WorkflowState;
    use crate::workflow::step::StepLog;

    fn wf_with(
        state: WorkflowState,
        steps_log: Vec<StepLog>,
        worktree: Option<PathBuf>,
    ) -> Workflow {
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
            pr_merged: false,
            cancel_token: CancellationToken::new(),
            worktree_lock: std::sync::Arc::new(tokio::sync::Mutex::new(())),
            current_def_total_steps: None,
            terminal_lines: Vec::new(),
            current_step_label: None,
            started_manually: false,
            jira_available: true,
            ticketing_available: true,
            ticketing_system: crate::config::TicketingSystem::Jira,
            ticket_url: None,
            last_session_id: None,
            description_session_id: None,
            driver_started: true,
            workflow_def_runs: std::collections::HashMap::new(),
            worktree_bootstrapped: false,
            worktree_preparing: false,
            workspace_name: "test-workspace".into(),
            repository_id: None,
            user_id: None,
            auth_pin: None,
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
    fn progress_fields_uses_persisted_count_for_completed() {
        let w = wf_with(WorkflowState::Done, vec![], None);
        let cfg = Config::default();
        // Completed + known count → that count at 100%.
        assert_eq!(progress_fields(&w, &cfg, Some(6)), (100, 6));
        assert_eq!(progress_fields(&w, &cfg, Some(10)), (100, 10));
    }

    #[test]
    fn progress_fields_completed_without_count_falls_back() {
        let w = wf_with(WorkflowState::Done, vec![], None);
        let cfg = Config::default();
        // No persisted count, or a zero count, → in-memory estimate (100% for Done).
        let (pct, total) = progress_fields(&w, &cfg, None);
        assert_eq!(pct, 100);
        assert!(total > 0);
        assert_eq!(
            progress_fields(&w, &cfg, Some(0)),
            progress_fields(&w, &cfg, None)
        );
    }

    #[test]
    fn progress_fields_ignores_count_for_non_completed() {
        let cfg = Config::default();
        // In-progress: the persisted count must not leak into the denominator.
        let running = wf_with(WorkflowState::AddressingTicket { pass: 1 }, vec![], None);
        assert_eq!(
            progress_fields(&running, &cfg, Some(99)),
            (
                workflow_progress_percent(&running, &cfg),
                estimated_step_total(&running, &cfg)
            )
        );
        // Errored terminal: never reported as a full/complete bar.
        let errored = wf_with(
            WorkflowState::Error {
                source_state: Box::new(WorkflowState::AddressingTicket { pass: 1 }),
                message: "boom".into(),
            },
            vec![],
            None,
        );
        let (pct, _) = progress_fields(&errored, &cfg, Some(6));
        assert_ne!(pct, 100);
    }

    #[test]
    fn filled_segments_rounds_percent() {
        assert_eq!(workflow_progress_filled_segments(0, 10), 0);
        assert_eq!(workflow_progress_filled_segments(100, 10), 10);
        assert_eq!(workflow_progress_filled_segments(74, 10), 7);
        assert_eq!(workflow_progress_filled_segments(75, 10), 8);
    }

    #[test]
    fn in_progress_workflow_estimates_more_steps() {
        let logs = vec![
            StepLog::new("Assign ticket".into()),
            StepLog::new("Retrieve details".into()),
            StepLog::new("Create worktree".into()),
            StepLog::new("Implement".into()),
        ];
        let w = wf_with(WorkflowState::AddressingTicket { pass: 1 }, logs, None);
        let cfg = Config::default();
        let total = estimated_step_total(&w, &cfg);
        // Should be > steps_log.len() since workflow is still in progress
        assert!(
            total > 4,
            "expected estimate to exceed logged steps for in-progress workflow, got {total}"
        );
    }
}
