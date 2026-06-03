// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Dynamic workflow-definition endpoints: list / run / retry.

use axum::Extension;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;

use maestro_core::workflow::definitions::{DiscoveredWorkflow, resolve_user_flows};

use crate::auth::AuthenticatedUser;
use crate::state::{AuthState, ConfigState, EngineState};

use super::require_workflow_access;

/// List the authenticated user's runnable workflow definitions for the active
/// workspace. Resolves through the per-user flow store (lazy-seeding from the
/// TOML defaults when the user has no row yet); there is no TOML fallback at
/// request time beyond that seed.
pub async fn list_workflow_definitions(
    State(auth_state): State<AuthState>,
    State(config): State<ConfigState>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Json<Vec<DiscoveredWorkflow>> {
    let Some(db) = auth_state.db.as_ref() else {
        return Json(Vec::new());
    };
    let workspace = config.active_workspace_name().await;
    let flows = resolve_user_flows(
        db,
        &auth.user_id,
        &workspace,
        &config.work_item_flow_defaults,
    )
    .await;
    Json(flows)
}

/// Start a specific workflow definition for a ticket.
pub async fn run_workflow_def(
    State(engine): State<EngineState>,
    State(auth_state): State<AuthState>,
    Path((id, def_name)): Path<(String, String)>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<StatusCode, (StatusCode, String)> {
    require_workflow_access(&engine, &auth_state, &auth, &id)
        .await
        .map_err(|s| (s, "Workflow not found".into()))?;
    engine
        .engine
        .start_workflow_def(&id, &def_name, Some(&auth.user_id))
        .await
        .map(|()| StatusCode::ACCEPTED)
        .map_err(|e| (StatusCode::CONFLICT, e.to_string()))
}

/// Retry a failed workflow definition for a ticket (resets Error -> Idle, then starts).
pub async fn retry_workflow_def(
    State(engine): State<EngineState>,
    State(auth_state): State<AuthState>,
    Path((id, def_name)): Path<(String, String)>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<StatusCode, (StatusCode, String)> {
    require_workflow_access(&engine, &auth_state, &auth, &id)
        .await
        .map_err(|s| (s, "Workflow not found".into()))?;
    engine
        .engine
        .retry_workflow_def(&id, &def_name, Some(&auth.user_id))
        .await
        .map(|()| StatusCode::ACCEPTED)
        .map_err(|e| (StatusCode::CONFLICT, e.to_string()))
}
