// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Dashboard "Add to Dashboard" / "Start manual workflow" endpoint.

use axum::Extension;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};

use maestro_core::workflow::state::WorkflowState;

use crate::auth::AuthenticatedUser;
use crate::state::{AuthState, ConfigState, EngineState};

#[derive(Deserialize)]
pub struct StartManualWorkflowBody {
    pub ticket_key: String,
    pub ticket_summary: String,
    /// Optional ticket description (used when Jira is unavailable and the user pastes the description).
    #[serde(default)]
    pub ticket_description: Option<String>,
    /// Direct URL to the issue in the ticketing system (e.g. GitHub issue `html_url`).
    /// Used so clicking the issue key on the dashboard opens the correct URL for GitHub workflows.
    #[serde(default)]
    pub issue_url: Option<String>,
    /// Id of a `repositories` row the caller has added. When omitted, the
    /// server picks the caller's most-recently-added repo (or rejects
    /// when the caller has none).
    #[serde(default)]
    pub repository_id: Option<String>,
}

#[derive(Serialize)]
pub struct StartManualWorkflowResponse {
    pub workflow_id: String,
    pub ticket_key: String,
}

/// Start a ticket workflow from the dashboard (same pipeline as the poller). Respects **`[general] max_concurrent_manual_workflows`**.
///
/// When Jira is unavailable (`jira_available = false`), `ticket_key` may be empty — a synthetic
/// `MANUAL-{timestamp}` key is generated. The `ticket_description` field is stored on the workflow
/// so the agent prompt can use it.
pub async fn start_manual_workflow(
    State(engine): State<EngineState>,
    State(auth_state): State<AuthState>,
    State(cfg): State<ConfigState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Json(body): Json<StartManualWorkflowBody>,
) -> Result<Json<StartManualWorkflowResponse>, (StatusCode, String)> {
    let jira_on = cfg
        .jira_available
        .load(std::sync::atomic::Ordering::Relaxed);

    let ticket_key = {
        let k = body.ticket_key.trim().to_string();
        if k.is_empty() {
            if jira_on {
                return Err((StatusCode::BAD_REQUEST, "ticket_key is required".into()));
            }
            // Auto-generate a synthetic key when Jira is unavailable.
            format!("MANUAL-{}", chrono::Utc::now().timestamp_millis())
        } else {
            k
        }
    };
    let ticket_summary = {
        let s = body.ticket_summary.trim();
        if s.is_empty() {
            if jira_on {
                ticket_key.clone()
            } else {
                "Manual item".to_string()
            }
        } else {
            s.to_string()
        }
    };

    let max_manual = {
        let cfg_guard = cfg.config.read().await;
        if jira_on && cfg_guard.jira.project_keys.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                "No Jira project keys configured".into(),
            ));
        }
        cfg_guard.general.max_concurrent_manual_workflows
    };

    {
        let wf_arc = engine.engine.workflows_arc();
        let map = wf_arc.read().await;
        if let Some(existing) = map.get(&ticket_key) {
            // Terminal-state entries (Done / Stopped / Error) are safe to replace —
            // the user is starting fresh on the same ticket. Replacement also recovers
            // from "orphan" rows (user_id = None) carried over from legacy snapshots:
            // those rows are invisible to the caller (per-user isolation), so without
            // this branch they would be undeletable zombies blocking the re-add.
            let terminal = matches!(
                existing.state,
                WorkflowState::Done | WorkflowState::Stopped | WorkflowState::Error { .. }
            );
            if !terminal {
                return Err((
                    StatusCode::CONFLICT,
                    format!("An item already exists for {ticket_key}"),
                ));
            }
            tracing::info!(
                ticket = %ticket_key,
                prev_state = %existing.state,
                prev_owner = ?existing.user_id,
                new_owner = %auth.user_id,
                "Replacing terminal-state workflow with a fresh add"
            );
        }
    }

    if max_manual > 0 {
        // Count per-user, not global.
        let wf_arc = engine.engine.workflows_arc();
        let map = wf_arc.read().await;
        let n = map
            .values()
            .filter(|w| w.user_id.as_deref() == Some(&auth.user_id))
            .filter(|w| w.started_manually && w.state.occupies_concurrency_slot())
            .count();
        if n >= max_manual as usize {
            return Err((
                StatusCode::CONFLICT,
                format!(
                    "Maximum concurrent manual items ({max_manual}) reached; complete, stop, or delete a manual item first"
                ),
            ));
        }
    }

    let description = body
        .ticket_description
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);

    let issue_url = body
        .issue_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);

    // Resolve the workflow's repository_id. When the body specifies one,
    // validate the caller has it associated; otherwise, default to the
    // most-recently-added repo. Reject when the caller has zero repos.
    let repository_id = if let Some(database) = auth_state.db.as_ref() {
        let user_repos =
            maestro_core::db::repositories::list_for_user(database.adapter(), &auth.user_id)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if user_repos.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                "Add a repository before starting an item.".into(),
            ));
        }
        let chosen_id = match body
            .repository_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(requested) => {
                if !user_repos.iter().any(|r| r.id == requested) {
                    return Err((
                        StatusCode::FORBIDDEN,
                        "You do not have access to that repository".into(),
                    ));
                }
                requested.to_string()
            }
            None => user_repos
                .iter()
                .max_by_key(|r| r.created_at)
                .map(|r| r.id.clone())
                // SAFETY: `user_repos.is_empty()` is rejected with 400
                // above, so the iterator has at least one element and
                // `max_by_key` returns `Some`.
                .expect("user_repos.is_empty() returned 400 above"),
        };
        Some(chosen_id)
    } else {
        // No DB attached (legacy test paths). Fall through with None — the
        // engine will derive workspace_name from cfg.git.repo_path.
        None
    };

    let workflow_id = engine
        .engine
        .add_to_dashboard(
            ticket_key.clone(),
            ticket_summary,
            true,
            description,
            issue_url,
            Some(auth.user_id),
            repository_id,
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(StartManualWorkflowResponse {
        workflow_id,
        ticket_key,
    }))
}
