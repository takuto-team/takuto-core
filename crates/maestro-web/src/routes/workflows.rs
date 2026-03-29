use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::Serialize;

use maestro_core::workflow::state::WorkflowState;
use maestro_core::workflow::step::StepLog;

use crate::state::AppState;

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
        .map(|w| WorkflowSummary {
            id: w.id.clone(),
            ticket_key: w.ticket_key.clone(),
            ticket_summary: w.ticket_summary.clone(),
            ticket_type: w.ticket_type.clone(),
            state: w.state.display_name(),
            started_at: w.started_at.to_rfc3339(),
            updated_at: w.updated_at.to_rfc3339(),
            branch_name: w.branch_name.clone(),
            pr_url: w.pr_url.clone(),
            steps_log: w.steps_log.clone(),
            error: extract_error(&w.state),
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
            Json(WorkflowSummary {
                id: w.id.clone(),
                ticket_key: w.ticket_key.clone(),
                ticket_summary: w.ticket_summary.clone(),
                ticket_type: w.ticket_type.clone(),
                state: w.state.display_name(),
                started_at: w.started_at.to_rfc3339(),
                updated_at: w.updated_at.to_rfc3339(),
                branch_name: w.branch_name.clone(),
                pr_url: w.pr_url.clone(),
                steps_log: w.steps_log.clone(),
                error: extract_error(&w.state),
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
