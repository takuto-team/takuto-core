// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Per-user, per-workspace work-item flow CRUD: `/api/me/flows`.
//!
//! Every endpoint scopes to the **active workspace**, derived server-side from
//! `config.git.repo_path` via [`crate::state::ConfigState::active_workspace_name`].
//! There is no client-supplied `?workspace=` parameter — switching workspace is
//! its own concern. The caller identity comes from the `AuthenticatedUser`
//! extension installed by the auth middleware; the user can only ever read or
//! write their own list.

use axum::Json;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};

use maestro_core::db::user_work_item_flows::{self, UserFlow};

use crate::auth::AuthenticatedUser;
use crate::state::{AuthState, ConfigState};

/// Response shape shared by all three endpoints: the caller's flow list plus
/// the workspace it is scoped to. An empty `flows` array is a valid response —
/// it means the user intentionally cleared their list.
#[derive(Debug, Serialize)]
pub struct FlowsResponse {
    pub flows: Vec<UserFlow>,
    pub workspace: String,
}

/// `PUT` body: the full replacement flow list.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutFlowsBody {
    pub flows: Vec<UserFlow>,
}

fn db(auth: &AuthState) -> Result<maestro_core::db::Database, (StatusCode, String)> {
    auth.db.clone().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "database unavailable".to_string(),
    ))
}

/// Map an internal db error to a response. A NUL byte is the caller's fault
/// (bad payload) → 400; everything else is a server fault → 500.
fn db_error(e: maestro_core::error::MaestroError) -> (StatusCode, String) {
    if let maestro_core::error::MaestroError::Db(maestro_core::db::DbError::NulByte { field }) = &e
    {
        return (
            StatusCode::BAD_REQUEST,
            format!("{field} contains a NUL byte"),
        );
    }
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

/// `GET /api/me/flows` — the caller's flow list for the active workspace.
///
/// Lazy-seeds from the TOML defaults when the row is absent (the user has
/// never been seeded for this workspace), so a freshly-reached workspace shows
/// flows on first load. An explicitly emptied list (`Some(vec![])`) is
/// returned as-is and never re-seeded.
pub async fn get_my_flows(
    State(auth): State<AuthState>,
    State(config): State<ConfigState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Result<Json<FlowsResponse>, (StatusCode, String)> {
    let db = db(&auth)?;
    let adapter = db.adapter();
    let workspace = config.active_workspace_name().await;

    let flows = match user_work_item_flows::get(adapter, &user.user_id, &workspace)
        .await
        .map_err(db_error)?
    {
        Some(flows) => flows,
        None => {
            // Not seeded yet for this workspace — seed once, then re-read.
            user_work_item_flows::seed_if_absent(
                adapter,
                &user.user_id,
                &workspace,
                &config.work_item_flow_defaults,
            )
            .await
            .map_err(db_error)?;
            user_work_item_flows::get(adapter, &user.user_id, &workspace)
                .await
                .map_err(db_error)?
                .unwrap_or_default()
        }
    };

    Ok(Json(FlowsResponse { flows, workspace }))
}

/// `PUT /api/me/flows` — replace the caller's full list for the active
/// workspace. Validated server-side; 400 with a structured `{error, kind}`
/// body on any validation failure. An empty list is allowed.
pub async fn put_my_flows(
    State(auth): State<AuthState>,
    State(config): State<ConfigState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(body): Json<PutFlowsBody>,
) -> Result<Json<FlowsResponse>, (StatusCode, String)> {
    let db = db(&auth)?;
    let adapter = db.adapter();
    let workspace = config.active_workspace_name().await;

    if let Err(err) = user_work_item_flows::validate_user_flows(&body.flows) {
        return Err((
            StatusCode::BAD_REQUEST,
            serde_json::json!({ "error": err.to_string(), "kind": err.kind() }).to_string(),
        ));
    }

    user_work_item_flows::set(adapter, &user.user_id, &workspace, &body.flows)
        .await
        .map_err(db_error)?;

    let flows = user_work_item_flows::get(adapter, &user.user_id, &workspace)
        .await
        .map_err(db_error)?
        .unwrap_or_default();

    tracing::info!(
        user_id = %user.user_id,
        workspace = %workspace,
        flow_count = flows.len(),
        "user work-item flows replaced"
    );

    Ok(Json(FlowsResponse { flows, workspace }))
}

/// `POST /api/me/flows/reseed` — destructively overwrite the caller's list for
/// the active workspace with the current TOML defaults. Returns the new list.
pub async fn reseed_my_flows(
    State(auth): State<AuthState>,
    State(config): State<ConfigState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> Result<Json<FlowsResponse>, (StatusCode, String)> {
    let db = db(&auth)?;
    let adapter = db.adapter();
    let workspace = config.active_workspace_name().await;

    let defaults = config.work_item_flow_defaults.as_ref().clone();
    user_work_item_flows::set(adapter, &user.user_id, &workspace, &defaults)
        .await
        .map_err(db_error)?;

    let flows = user_work_item_flows::get(adapter, &user.user_id, &workspace)
        .await
        .map_err(db_error)?
        .unwrap_or_default();

    tracing::info!(
        user_id = %user.user_id,
        workspace = %workspace,
        flow_count = flows.len(),
        "user work-item flows re-seeded from defaults"
    );

    Ok(Json(FlowsResponse { flows, workspace }))
}
