use std::path::PathBuf;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use maestro_core::claude::session::ClaudeSession;
use maestro_core::config::TicketingSystem;
use maestro_core::jira::client::JiraClient;

use crate::routes::github::parse_github_repo;
use crate::state::AppState;

const IMPROVE_SYSTEM_PROMPT: &str = "\
You are a technical writer who improves software ticket descriptions. \
Output ONLY the improved description in Markdown format. \
Do not add any preamble, commentary, explanation, or closing remarks.";

#[derive(Deserialize)]
pub struct ImproveTicketBody {
    pub description: String,
    pub summary: String,
}

#[derive(Serialize)]
pub struct ImproveTicketResponse {
    pub improved_description: String,
}

#[derive(Deserialize)]
pub struct UpdateDescriptionBody {
    pub description: String,
    /// Optional summary/title update (used in no-ticketing mode).
    #[serde(default)]
    pub summary: Option<String>,
}

/// Maximum allowed length for the ticket description in an improve request (100 KB).
const MAX_IMPROVE_DESCRIPTION_LEN: usize = 100 * 1024;

/// `POST /api/tickets/{key}/improve` — run a headless Claude session to improve the ticket description.
pub async fn improve_ticket(
    State(state): State<AppState>,
    Path(key): Path<String>,
    Json(body): Json<ImproveTicketBody>,
) -> Result<Json<ImproveTicketResponse>, (StatusCode, String)> {
    if body.description.len() > MAX_IMPROVE_DESCRIPTION_LEN {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "Description exceeds maximum allowed length ({} bytes, limit {})",
                body.description.len(),
                MAX_IMPROVE_DESCRIPTION_LEN
            ),
        ));
    }

    let model = {
        let config = state.config.read().await;
        let m = config.agent.model.trim().to_string();
        if m.is_empty() { None } else { Some(m) }
    };

    let prompt = format!(
        "Improve the following ticket description. Make it clearer, more actionable, and \
technically precise. Add acceptance criteria if none are present. Keep the original intent intact.\n\n\
**Ticket:** {key} — {summary}\n\n\
**Current description:**\n{description}",
        key = key,
        summary = body.summary,
        description = body.description,
    );

    // Use the system temp dir rather than a hardcoded "/tmp" for portability.
    let worktree = std::env::temp_dir();

    let session = ClaudeSession::run_prompt(
        &worktree,
        &prompt,
        CancellationToken::new(),
        300, // 5 minutes — generous for a single LLM generation
        None,
        model.as_deref(),
        None,
        None,
        Some(IMPROVE_SYSTEM_PROMPT),
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(ImproveTicketResponse {
        improved_description: session.output,
    }))
}

/// `POST /api/tickets/{key}/update-description` — persist the improved description to the ticketing system.
pub async fn update_ticket_description(
    State(state): State<AppState>,
    Path(key): Path<String>,
    Json(body): Json<UpdateDescriptionBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    match state.ticketing_system {
        TicketingSystem::None => {
            // No external ticketing system — persist to the in-memory workflow.
            let mut workflows = state.engine.workflows.write().await;
            if let Some(wf) = workflows.get_mut(&key) {
                wf.ticket_description = body.description.clone();
                if let Some(ref s) = body.summary {
                    wf.ticket_summary = s.clone();
                }
            }
            drop(workflows);
            // Best-effort snapshot sync so the edit survives a restart.
            let _ = state.engine.sync_workflow_snapshot().await;
            Ok(Json(serde_json::json!({})))
        }
        TicketingSystem::GitHub => {
            let repo_url = {
                let config = state.config.read().await;
                config.git.repo_url.clone()
            };
            let owner_repo = parse_github_repo(&repo_url).ok_or_else(|| {
                (
                    StatusCode::BAD_REQUEST,
                    format!("Cannot parse GitHub owner/repo from git.repo_url: {repo_url:?}"),
                )
            })?;
            let issue_number: u64 = key
                .strip_prefix("GH-")
                .and_then(|n| n.parse().ok())
                .ok_or_else(|| {
                    (
                        StatusCode::BAD_REQUEST,
                        format!("Cannot parse GitHub issue number from key: {key:?}"),
                    )
                })?;

            let output = tokio::process::Command::new("gh")
                .args([
                    "api",
                    "--method",
                    "PATCH",
                    &format!("repos/{owner_repo}/issues/{issue_number}"),
                    // --raw-field sends the value as a plain string, avoiding gh's
                    // type-coercion of "true"/"false"/"123" in user-supplied text.
                    "--raw-field",
                    &format!("body={}", body.description),
                ])
                .output()
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err((
                    StatusCode::BAD_GATEWAY,
                    format!("gh api PATCH issues/{issue_number} failed: {stderr}"),
                ));
            }

            Ok(Json(serde_json::json!({})))
        }
        TicketingSystem::Jira => {
            let (repo_path, acli_extras) = {
                let config = state.config.read().await;
                (
                    PathBuf::from(&config.git.repo_path),
                    config.jira.acli_extra_argv_prefixes(),
                )
            };
            let client = JiraClient::new(repo_path, acli_extras);
            client
                .update_description(&key, &body.description)
                .await
                .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;
            Ok(Json(serde_json::json!({})))
        }
    }
}
