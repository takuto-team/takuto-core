// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Per-user, per-workspace work-item flow CRUD: `/api/me/flows`.
//!
//! Every endpoint scopes to a workspace. The optional `?workspace=<name>` query
//! param selects it (the Workflows settings tab passes the repo the user picked
//! in its sidebar); when absent or blank it falls back to the **active
//! workspace**, derived server-side from `config.git.repo_path` via
//! [`crate::state::ConfigState::active_workspace_name`]. Rows are keyed by
//! `(user_id, workspace)`, so the caller can only ever read or write their own
//! list — the identity comes from the `AuthenticatedUser` extension installed
//! by the auth middleware.

use axum::Json;
use axum::extract::{Extension, Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};

use takuto_core::db::user_work_item_flows::{self, UserFlow};

use crate::auth::AuthenticatedUser;
use crate::state::{AuthState, ConfigState, EngineState};

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

/// Query string for all three flow endpoints. `workspace` selects which repo's
/// flows to read/write; absent or blank → the active workspace.
#[derive(Debug, Default, Deserialize)]
pub struct FlowsQuery {
    pub workspace: Option<String>,
}

/// Resolve the target workspace: the trimmed `?workspace=` when non-empty, else
/// the server's active workspace.
async fn resolve_workspace(q: &FlowsQuery, config: &ConfigState) -> String {
    match q.workspace.as_deref().map(str::trim) {
        Some(w) if !w.is_empty() => w.to_string(),
        _ => config.active_workspace_name().await,
    }
}

fn db(auth: &AuthState) -> Result<takuto_core::db::Database, (StatusCode, String)> {
    auth.db.clone().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "database unavailable".to_string(),
    ))
}

/// Map an internal db error to a response. A NUL byte is the caller's fault
/// (bad payload) → 400 with the same `{error, kind}` structured body as every
/// other validation failure; everything else is a server fault → 500.
fn db_error(e: takuto_core::error::TakutoError) -> (StatusCode, String) {
    if let takuto_core::error::TakutoError::Db(takuto_core::db::DbError::NulByte { field }) = &e {
        return (
            StatusCode::BAD_REQUEST,
            serde_json::json!({
                "error": format!("{field} contains a NUL byte"),
                "kind": "nul_byte",
            })
            .to_string(),
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
    Query(q): Query<FlowsQuery>,
) -> Result<Json<FlowsResponse>, (StatusCode, String)> {
    let db = db(&auth)?;
    let adapter = db.adapter();
    let workspace = resolve_workspace(&q, &config).await;

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
    Query(q): Query<FlowsQuery>,
    Json(body): Json<PutFlowsBody>,
) -> Result<Json<FlowsResponse>, (StatusCode, String)> {
    let db = db(&auth)?;
    let adapter = db.adapter();
    let workspace = resolve_workspace(&q, &config).await;

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
    Query(q): Query<FlowsQuery>,
) -> Result<Json<FlowsResponse>, (StatusCode, String)> {
    let db = db(&auth)?;
    let adapter = db.adapter();
    let workspace = resolve_workspace(&q, &config).await;

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

const IMPROVE_PROMPT_SYSTEM_PROMPT: &str = "\
You are a technical writer who improves prompts that will be sent to a coding agent. \
Output ONLY the improved prompt, in Markdown, with no preamble or commentary. \
Make it clearer, more actionable, and more precise. Keep the original intent intact.";

/// Hard cap on the user-supplied text in an improve request (100 KB), mirroring
/// the ticket-description improve path.
const MAX_IMPROVE_PROMPT_LEN: usize = 100 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImprovePromptBody {
    pub text: String,
}

#[derive(Debug, Serialize)]
pub struct ImprovePromptResponse {
    pub improved_text: String,
}

/// `POST /api/me/text/improve` — improve an arbitrary chunk of user-authored
/// text (currently used for flow step prompts) via the configured AI agent.
/// Mirrors the `improve_ticket` pipeline but without a workflow context: each
/// call runs a fresh session in an ephemeral worker container.
pub async fn improve_prompt(
    State(engine): State<EngineState>,
    State(auth): State<AuthState>,
    State(config): State<ConfigState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(body): Json<ImprovePromptBody>,
) -> Result<Json<ImprovePromptResponse>, (StatusCode, String)> {
    if body.text.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "text must not be empty".to_string(),
        ));
    }
    if body.text.len() > MAX_IMPROVE_PROMPT_LEN {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "Text exceeds maximum allowed length ({} bytes, limit {})",
                body.text.len(),
                MAX_IMPROVE_PROMPT_LEN,
            ),
        ));
    }

    // Synthetic key: the `run_description_session` helper uses it only to look
    // up an optional resume session and to name the ephemeral container. The
    // workflow map will return `None` for an unknown key, so each improve call
    // runs a fresh session.
    let synthetic_key = format!("flow-prompt-{}", uuid::Uuid::new_v4());
    let prompt = format!(
        "Improve the following prompt that will be sent to a coding agent. \
Make it clearer, more actionable, and more precise. Keep the original intent intact.\n\n\
**Current prompt:**\n{}",
        body.text,
    );

    let output = crate::routes::tickets::run_description_session(
        &engine,
        &auth,
        &config,
        &synthetic_key,
        &user.user_id,
        &prompt,
        Some(IMPROVE_PROMPT_SYSTEM_PROMPT),
    )
    .await?;

    Ok(Json(ImprovePromptResponse {
        improved_text: output.trim().to_string(),
    }))
}
