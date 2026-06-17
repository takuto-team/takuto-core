// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Dynamic workflow-definition endpoints: list / run / retry.

use axum::Extension;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;

use takuto_core::workflow::definitions::{DiscoveredWorkflow, resolve_user_flows};

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

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use crate::server::build_router;
    use crate::test_helpers::{TEST_ORIGIN, register_and_login, test_state_with_db};

    /// `list_workflow_definitions` resolves the per-user flow store and returns
    /// a JSON array. With the default (empty) flow defaults the body is `[]`,
    /// but the handler still exercises the full DB-backed resolve path.
    #[tokio::test]
    async fn list_definitions_returns_json_array_for_authed_user() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;
        let app = build_router(state);

        let resp = app
            .oneshot(
                Request::get("/api/workflow-definitions")
                    .header("Cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json.is_array(), "definitions response must be a JSON array");
    }

    /// Running a definition for a ticket the caller doesn't own (here: a
    /// non-existent workflow) is rejected by `require_workflow_access` with
    /// 404 — the access guard, not the engine, is what answers.
    #[tokio::test]
    async fn run_workflow_def_404_for_unknown_workflow() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;
        let app = build_router(state);

        let resp = app
            .oneshot(
                Request::post("/api/workflows/NOPE-1/run-workflow/implement")
                    .header("Origin", TEST_ORIGIN)
                    .header("Cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn retry_workflow_def_404_for_unknown_workflow() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;
        let app = build_router(state);

        let resp = app
            .oneshot(
                Request::post("/api/workflows/NOPE-1/retry-workflow/implement")
                    .header("Origin", TEST_ORIGIN)
                    .header("Cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
