// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::path::PathBuf;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use maestro_core::claude::session::ClaudeSession;
use maestro_core::config::{AiAgentProvider, TicketingSystem, cursor_model_for_cli};
use maestro_core::cursor::session::CursorSession;
use maestro_core::jira::client::JiraClient;

use crate::routes::github::parse_github_repo;
use crate::state::AppState;

const IMPROVE_SYSTEM_PROMPT: &str = "\
You are a technical writer who improves software ticket descriptions. \
Output the improved title on the FIRST line, then a line containing only `---`, \
then the improved description in Markdown format. Example:\n\
Improved Title Here\n\
---\n\
Improved description in Markdown...\n\n\
You may use Mermaid diagram blocks (```mermaid) when a visual flowchart, \
sequence diagram, or architecture diagram would clarify the description. \
Do not add any preamble, commentary, explanation, or closing remarks.";

#[derive(Deserialize)]
pub struct ImproveTicketBody {
    pub description: String,
    pub summary: String,
    /// Optional extra instructions from the user (e.g. "add acceptance criteria").
    #[serde(default)]
    pub prompt: Option<String>,
}

#[derive(Serialize)]
pub struct ImproveTicketResponse {
    pub improved_description: String,
    /// Improved title suggested by the AI (empty if parsing failed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub improved_summary: Option<String>,
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

/// Run a description-editing AI session (improve / prompt) using the configured provider.
///
/// Reads the workflow's `description_session_id` to resume the shared conversation, then
/// writes the new session ID back so the next call continues in the same context.
/// Falls back gracefully when the ticket has no associated workflow in the map.
async fn run_description_session(
    state: &AppState,
    ticket_key: &str,
    prompt: &str,
    system_prompt: Option<&str>,
) -> Result<String, (StatusCode, String)> {
    // Snapshot the config fields we need before any await point.
    let (provider, model, cursor_cli, cursor_model) = {
        let cfg = state.config.read().await;
        (
            cfg.agent.provider,
            if cfg.agent.model.trim().is_empty() {
                None
            } else {
                Some(cfg.agent.model.trim().to_string())
            },
            cfg.agent.cursor_cli.clone(),
            cfg.agent.cursor_model.clone(),
        )
    };

    // Read the persisted description session ID for this workflow (if any).
    let resume_id: Option<String> = {
        let wf = state.engine.workflows.read().await;
        wf.get(ticket_key)
            .and_then(|w| w.description_session_id.clone())
    };

    let worktree = std::env::temp_dir();
    let cancel = CancellationToken::new();

    // Run with the configured provider.
    let (session_id, output) = match provider {
        AiAgentProvider::Claude => {
            let sess = ClaudeSession::run_prompt(
                &worktree,
                prompt,
                cancel,
                300,
                None,
                model.as_deref(),
                resume_id.as_deref(),
                None,
                // System prompt is only effective on a fresh session; on resume the existing
                // session already has its system prompt — inject it into the user message instead.
                if resume_id.is_some() { None } else { system_prompt },
            )
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            (sess.session_id, sess.output)
        }
        AiAgentProvider::Cursor => {
            let effective_model = cursor_model_for_cli(&cursor_model);
            let sess = CursorSession::run_prompt(
                &cursor_cli,
                &worktree,
                prompt,
                cancel,
                300,
                None,
                if effective_model == "Auto" {
                    None
                } else {
                    Some(effective_model)
                },
                resume_id.as_deref(),
                None,
            )
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            (sess.session_id, sess.output)
        }
    };

    // Persist the session ID back to the workflow so the next call resumes it.
    {
        let mut wf = state.engine.workflows.write().await;
        if let Some(w) = wf.get_mut(ticket_key) {
            w.description_session_id = Some(session_id);
        }
    }
    // Best-effort snapshot so the session survives a restart.
    let _ = state.engine.sync_workflow_snapshot().await;

    Ok(output)
}

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

    let mut prompt = format!(
        "Improve the following ticket description. Make it clearer, more actionable, and \
technically precise. Add acceptance criteria if none are present. Keep the original intent intact.\n\n\
**Ticket:** {key} — {summary}\n\n\
**Current description:**\n{description}",
        key = key,
        summary = body.summary,
        description = body.description,
    );
    if let Some(extra) = &body.prompt {
        if !extra.trim().is_empty() {
            prompt.push_str(&format!("\n\n**Additional instructions:** {extra}"));
        }
    }

    let output =
        run_description_session(&state, &key, &prompt, Some(IMPROVE_SYSTEM_PROMPT)).await?;

    // Parse "Title\n---\nDescription" format from AI output.
    let (improved_summary, improved_description) =
        if let Some((before, after)) = output.split_once("\n---\n") {
            let title = before.trim().to_string();
            let desc = after.trim().to_string();
            if title.is_empty() {
                (None, output.clone())
            } else {
                (Some(title), desc)
            }
        } else {
            (None, output.clone())
        };

    Ok(Json(ImproveTicketResponse {
        improved_description,
        improved_summary,
    }))
}

/// Maximum allowed length for the user prompt in a prompt request (10 KB).
const MAX_PROMPT_LEN: usize = 10 * 1024;

const PROMPT_SYSTEM_PROMPT: &str = "\
You are a technical assistant helping a user refine a software ticket. \
The user will provide custom instructions; answer them directly. \
Use Markdown formatting. You may use Mermaid diagram blocks (```mermaid) when \
a visual diagram would clarify your answer. \
Do not add any preamble, commentary, or closing remarks — output only the requested content.";

#[derive(Deserialize)]
pub struct PromptTicketBody {
    pub prompt: String,
    pub ticket_title: String,
    pub ticket_description: String,
}

#[derive(Serialize)]
pub struct PromptTicketResponse {
    pub response: String,
}

/// `POST /api/tickets/{key}/prompt` — run a headless Claude session with a custom user prompt and ticket context.
pub async fn prompt_ticket(
    State(state): State<AppState>,
    Path(key): Path<String>,
    Json(body): Json<PromptTicketBody>,
) -> Result<Json<PromptTicketResponse>, (StatusCode, String)> {
    // Validate prompt is not empty
    if body.prompt.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "Prompt must not be empty".to_string(),
        ));
    }

    // Validate prompt length
    if body.prompt.len() > MAX_PROMPT_LEN {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "Prompt exceeds maximum allowed length ({} bytes, limit {})",
                body.prompt.len(),
                MAX_PROMPT_LEN
            ),
        ));
    }

    // Validate description length (same limit as improve endpoint)
    if body.ticket_description.len() > MAX_IMPROVE_DESCRIPTION_LEN {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "Ticket description exceeds maximum allowed length ({} bytes, limit {})",
                body.ticket_description.len(),
                MAX_IMPROVE_DESCRIPTION_LEN
            ),
        ));
    }

    let prompt = format!(
        "{user_prompt}\n\n\
         **Ticket:** {key} — {title}\n\n\
         **Current description:**\n{description}",
        user_prompt = body.prompt,
        key = key,
        title = body.ticket_title,
        description = body.ticket_description,
    );

    let output =
        run_description_session(&state, &key, &prompt, Some(PROMPT_SYSTEM_PROMPT)).await?;

    Ok(Json(PromptTicketResponse { response: output }))
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

            let mut gh_args = vec![
                "api".to_string(),
                "--method".to_string(),
                "PATCH".to_string(),
                format!("repos/{owner_repo}/issues/{issue_number}"),
                "--raw-field".to_string(),
                format!("body={}", body.description),
            ];
            if let Some(ref s) = body.summary {
                gh_args.push("--raw-field".to_string());
                gh_args.push(format!("title={s}"));
            }
            let output = tokio::process::Command::new("gh")
                .args(&gh_args)
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

            // Update in-memory summary (for dashboard card title) but NOT description —
            // GitHub is the authoritative source; next "Show description" fetches fresh.
            if let Some(ref s) = body.summary {
                let mut workflows = state.engine.workflows.write().await;
                if let Some(wf) = workflows.get_mut(&key) {
                    wf.ticket_summary = s.clone();
                }
                drop(workflows);
                let _ = state.engine.sync_workflow_snapshot().await;
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

            // Update in-memory summary (for dashboard card title) but NOT description —
            // Jira is the authoritative source; next "Show description" fetches fresh.
            if let Some(ref s) = body.summary {
                let mut workflows = state.engine.workflows.write().await;
                if let Some(wf) = workflows.get_mut(&key) {
                    wf.ticket_summary = s.clone();
                }
                drop(workflows);
                let _ = state.engine.sync_workflow_snapshot().await;
            }

            Ok(Json(serde_json::json!({})))
        }
    }
}
