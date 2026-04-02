use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Serialize;

use maestro_core::workflow::engine::{MarkDoneOutcome, TerminalLine, Workflow};
use maestro_core::workflow::state::WorkflowState;
use maestro_core::workflow::step::StepLog;

use crate::state::AppState;

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
    pub ticket_type: String,
    pub state: String,
    pub started_at: String,
    pub updated_at: String,
    pub branch_name: String,
    pub pr_url: Option<String>,
    pub steps_log: Vec<StepLog>,
    pub error: Option<String>,
    pub terminal_lines: Vec<TerminalLineDto>,
    /// **Address PR Comments** is allowed (main flow **Done** and `pr_url` set).
    pub can_address_pr_comments: bool,
    /// **Merge base branch** is allowed (main flow **Done**, `pr_url` set, worktree exists).
    pub can_merge_base: bool,
    /// **Mark as Done** is allowed (workflow state is **Done**).
    pub can_mark_done: bool,
}

fn workflow_action_flags(w: &Workflow) -> (bool, bool, bool) {
    let done = matches!(w.state, WorkflowState::Done);
    let has_pr = w
        .pr_url
        .as_deref()
        .map(str::trim)
        .is_some_and(|s| !s.is_empty());
    let has_worktree = w
        .worktree_path
        .as_ref()
        .is_some_and(|p| p.exists());
    let can_address = done && has_pr;
    let can_merge_base = done && has_pr && has_worktree;
    let can_mark = done;
    (can_address, can_merge_base, can_mark)
}

fn extract_error(state: &WorkflowState) -> Option<String> {
    match state {
        WorkflowState::Error { message, .. } => Some(message.clone()),
        _ => None,
    }
}

pub async fn list_workflows(State(state): State<AppState>) -> Json<Vec<WorkflowSummary>> {
    let workflows = state.engine.workflows.read().await;
    let mut summaries: Vec<WorkflowSummary> = workflows
        .values()
        .map(|w| {
            let (can_address_pr_comments, can_merge_base, can_mark_done) = workflow_action_flags(w);
            WorkflowSummary {
                id: w.id.clone(),
                ticket_key: w.ticket_key.clone(),
                ticket_summary: w.ticket_summary.clone(),
                ticket_type: w.ticket_type.clone(),
                state: w.status_display(),
                started_at: w.started_at.to_rfc3339(),
                updated_at: w.updated_at.to_rfc3339(),
                branch_name: w.branch_name.clone(),
                pr_url: w.pr_url.clone(),
                steps_log: w.steps_log.clone(),
                error: extract_error(&w.state),
                terminal_lines: w.terminal_lines.iter().map(TerminalLineDto::from).collect(),
                can_address_pr_comments,
                can_merge_base,
                can_mark_done,
            }
        })
        .collect();
    // Newest first
    summaries.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    Json(summaries)
}

pub async fn get_workflow(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<WorkflowSummary>, StatusCode> {
    let workflows = state.engine.workflows.read().await;
    workflows
        .get(&id)
        .map(|w| {
            let (can_address_pr_comments, can_merge_base, can_mark_done) = workflow_action_flags(w);
            Json(WorkflowSummary {
                id: w.id.clone(),
                ticket_key: w.ticket_key.clone(),
                ticket_summary: w.ticket_summary.clone(),
                ticket_type: w.ticket_type.clone(),
                state: w.status_display(),
                started_at: w.started_at.to_rfc3339(),
                updated_at: w.updated_at.to_rfc3339(),
                branch_name: w.branch_name.clone(),
                pr_url: w.pr_url.clone(),
                steps_log: w.steps_log.clone(),
                error: extract_error(&w.state),
                terminal_lines: w.terminal_lines.iter().map(TerminalLineDto::from).collect(),
                can_address_pr_comments,
                can_merge_base,
                can_mark_done,
            })
        })
        .ok_or(StatusCode::NOT_FOUND)
}

/// Pause a running workflow. Delegates to WorkflowEngine::pause_workflow
/// which sets Paused state and broadcasts a WebSocket event.
pub async fn pause_workflow(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .engine
        .pause_workflow(&id)
        .await
        .map(|()| StatusCode::OK)
        .map_err(|e| (StatusCode::CONFLICT, e.to_string()))
}

/// Resume a paused workflow. Delegates to WorkflowEngine::resume_workflow
/// which restores the source state and broadcasts a WebSocket event.
/// The drive_workflow loop's wait_if_paused will detect the un-pause and continue.
pub async fn resume_workflow(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .engine
        .resume_workflow(&id)
        .await
        .map(|()| StatusCode::OK)
        .map_err(|e| (StatusCode::CONFLICT, e.to_string()))
}

/// Retry a failed/stopped/completed workflow. Removes the old workflow and starts fresh.
pub async fn retry_workflow(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .engine
        .retry_workflow(&id)
        .await
        .map(|_| StatusCode::OK)
        .map_err(|e| (StatusCode::CONFLICT, e.to_string()))
}

/// Stop a workflow. Delegates to WorkflowEngine::stop_workflow which:
/// - Cancels the CancellationToken (killing running processes)
/// - Sets Stopped state
/// - Spawns a fire-and-forget task to unassign the Jira ticket and move it back to "To Do"
/// - Broadcasts a WebSocket event
pub async fn stop_workflow(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .engine
        .stop_workflow(&id)
        .await
        .map(|()| StatusCode::OK)
        .map_err(|e| (StatusCode::CONFLICT, e.to_string()))
}

/// Run the configured **`[[review_agent_steps]]`** sequence in the existing worktree (requires **Done** + PR URL).
pub async fn address_pr_comments(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .engine
        .start_pr_review_workflow(&id)
        .await
        .map(|()| StatusCode::ACCEPTED)
        .map_err(|e| (StatusCode::CONFLICT, e.to_string()))
}

/// Run the configured **`[[merge_base_agent_steps]]`** sequence in the existing worktree (requires **Done** + PR URL).
pub async fn merge_base_branch(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .engine
        .start_merge_base_workflow(&id)
        .await
        .map(|()| StatusCode::ACCEPTED)
        .map_err(|e| (StatusCode::CONFLICT, e.to_string()))
}

/// Jira transition to configured **Done** status and remove worktree; removes the workflow on full success.
pub async fn mark_work_done(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<MarkDoneOutcome>, (StatusCode, String)> {
    state
        .engine
        .mark_work_done(&id)
        .await
        .map(Json)
        .map_err(|e| (StatusCode::CONFLICT, e.to_string()))
}
