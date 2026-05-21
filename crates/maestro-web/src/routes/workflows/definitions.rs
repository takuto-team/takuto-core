// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Dynamic workflow-definition endpoints: list / run / retry.

use axum::Extension;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;

use maestro_core::workflow::definitions::{DiscoveredWorkflow, discover_workflows};

use crate::auth::AuthenticatedUser;
use crate::state::AppState;

use super::require_workflow_access;

/// List all discovered workflow definitions from the workflows directory.
pub async fn list_workflow_definitions(
    State(state): State<AppState>,
) -> Json<Vec<DiscoveredWorkflow>> {
    let dir = state.engine.workflows_dir.clone();
    let result = discover_workflows(&dir);
    Json(result.workflows)
}

/// Start a specific workflow definition for a ticket.
pub async fn run_workflow_def(
    State(state): State<AppState>,
    Path((id, def_name)): Path<(String, String)>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<StatusCode, (StatusCode, String)> {
    require_workflow_access(&state, &auth, &id)
        .await
        .map_err(|s| (s, "Workflow not found".into()))?;
    state
        .engine
        .start_workflow_def(&id, &def_name)
        .await
        .map(|()| StatusCode::ACCEPTED)
        .map_err(|e| (StatusCode::CONFLICT, e.to_string()))
}

/// Retry a failed workflow definition for a ticket (resets Error -> Idle, then starts).
pub async fn retry_workflow_def(
    State(state): State<AppState>,
    Path((id, def_name)): Path<(String, String)>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<StatusCode, (StatusCode, String)> {
    require_workflow_access(&state, &auth, &id)
        .await
        .map_err(|s| (s, "Workflow not found".into()))?;
    state
        .engine
        .retry_workflow_def(&id, &def_name)
        .await
        .map(|()| StatusCode::ACCEPTED)
        .map_err(|e| (StatusCode::CONFLICT, e.to_string()))
}
