// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::path::PathBuf;

use axum::Json;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use takuto_core::claude::session::ClaudeSession;
use takuto_core::codex::CodexSession;
use takuto_core::config::{AiAgentProvider, TicketingSystem, cursor_model_for_cli};
use takuto_core::container::ContainerRunner;
use takuto_core::cursor::session::CursorSession;
use takuto_core::jira::client::JiraClient;
use takuto_core::jira::{JiraRestClient, resolve_rest_credential};
use takuto_core::opencode::OpenCodeSession;

use crate::auth::AuthenticatedUser;
use crate::routes::github::parse_github_repo;
use crate::routes::jira::{JiraRouteError, map_jira_err};
use crate::routes::workflows::require_workflow_access;
use crate::state::{AuthState, ConfigState, EngineState};

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
    /// Repo name (`repositories.name`) the ticket belongs to. Used only when the
    /// ticket is **not yet a work item** (the Add-to-Dashboard preview flow) to
    /// resolve which GitHub repo's issue to PATCH; ignored once a work item
    /// exists (the work item's own repo wins). Irrelevant for Jira (host-level
    /// acli) and None mode.
    #[serde(default)]
    pub repository: Option<String>,
}

/// Maximum allowed length for the ticket description in an improve request (100 KB).
const MAX_IMPROVE_DESCRIPTION_LEN: usize = 100 * 1024;

/// Resolve the on-disk `local_path` of the repository associated with the
/// workflow keyed by `ticket_key`. Falls back to `cfg.git.repo_path` when
/// the workflow has no `repository_id` (legacy snapshots) or when no DB is
/// attached (test paths).
async fn resolve_workflow_repo_path(
    engine: &EngineState,
    auth_state: &AuthState,
    cfg_state: &ConfigState,
    ticket_key: &str,
    // Repo-name hint for tickets that aren't a work item yet (preview flow).
    // Used only when no work item supplies a repo.
    fallback_repo_name: Option<&str>,
) -> PathBuf {
    let (repo_id, ws_name) = {
        let wf_arc = engine.engine.workflows_arc();
        let wf = wf_arc.read().await;
        wf.get(ticket_key)
            .map(|w| (w.repository_id.clone(), w.workspace_name.clone()))
            .unwrap_or_default()
    };
    if let Some(database) = auth_state.db.as_ref() {
        let adapter = database.adapter();
        if let Some(id) = repo_id.as_deref()
            && let Ok(Some(row)) = takuto_core::db::repositories::get(adapter, id).await
        {
            return PathBuf::from(&row.local_path);
        }
        if !ws_name.is_empty()
            && let Ok(Some(row)) =
                takuto_core::db::repositories::get_by_name(adapter, &ws_name).await
        {
            return PathBuf::from(&row.local_path);
        }
        // No work item repo — fall back to the caller-supplied repo (preview flow).
        if let Some(name) = fallback_repo_name.filter(|n| !n.is_empty())
            && let Ok(Some(row)) = takuto_core::db::repositories::get_by_name(adapter, name).await
        {
            return PathBuf::from(&row.local_path);
        }
    }
    let cfg = cfg_state.config.read().await;
    PathBuf::from(&cfg.git.repo_path)
}

/// The env var an agent CLI reads its API key from (env-injectable providers).
/// OpenCode reads a generated config file, not an env var → `None`.
fn provider_api_key_env(provider: AiAgentProvider) -> Option<&'static str> {
    match provider {
        AiAgentProvider::Claude => Some("CLAUDE_CODE_OAUTH_TOKEN"),
        AiAgentProvider::Cursor => Some("CURSOR_API_KEY"),
        AiAgentProvider::Codex => Some("OPENAI_API_KEY"),
        AiAgentProvider::OpenCode => None,
    }
}

/// Resolve the caller's own provider API key as a `(VAR, value)` env pair for a
/// main-container agent run. `None` → the user has no per-user key (or the
/// provider isn't env-injectable / no master key) → the CLI inherits the main
/// container's ambient (app-instance) credential. The value is returned by-value
/// (per call), so concurrent invocations never share mutable global env.
async fn resolve_user_credential_env(
    auth_state: &AuthState,
    provider: AiAgentProvider,
    user_id: &str,
) -> Option<(String, String)> {
    let var = provider_api_key_env(provider)?;
    let db = auth_state.db.as_ref()?;
    let master = db.master_key()?;
    let row = takuto_core::db::provider_credentials::find_active_with_kind(
        db.adapter(),
        user_id,
        provider.as_str(),
        takuto_core::db::provider_credentials::ProviderCredentialKind::ApiKey,
    )
    .await
    .ok()??;
    let sealed = takuto_core::auth::SealedBlob {
        ciphertext: row.ciphertext,
        nonce: row.nonce,
        wrapped_dek: row.wrapped_dek,
        wnonce: row.wnonce,
    };
    let plaintext = takuto_core::auth::open(&master.key, &sealed).ok()?;
    let value = String::from_utf8(plaintext).ok()?;
    Some((var.to_string(), value))
}

/// Run a description-editing AI session (improve / prompt) using the configured provider.
///
/// Runs the agent CLI directly in the main container with the caller's own provider
/// credential (else the app-instance default) injected per-process. Reads the
/// workflow's `description_session_id` to resume the shared conversation, then writes
/// the new session ID back so the next call continues in the same context. Falls back
/// gracefully when the ticket has no associated workflow in the map — for non-ticket
/// improve callers (e.g. flow step-prompt improvement) pass any unique key; the
/// workflow lookup returns `None` and the session runs fresh each time.
pub(crate) async fn run_description_session(
    engine: &EngineState,
    auth_state: &AuthState,
    cfg_state: &ConfigState,
    ticket_key: &str,
    user_id: &str,
    prompt: &str,
    system_prompt: Option<&str>,
) -> Result<String, (StatusCode, String)> {
    // Snapshot the config fields we need before any await point.
    let (provider, model, cursor_cli, cursor_model, codex_model, opencode_model, improve_timeout) = {
        let cfg = cfg_state.config.read().await;
        (
            cfg.agent.provider,
            // Route only feeds `model` into the Claude branch
            // (`AiAgentProvider::Claude` → `ClaudeSession::run_prompt`), so
            // resolve via the accessor: an empty `[agent.providers.claude].model`
            // must omit `--model` rather than force a stale value.
            cfg.agent.effective_claude_model().map(str::to_string),
            cfg.agent.effective_cursor_cli().to_string(),
            cfg.agent.effective_cursor_model().to_string(),
            {
                let m = cfg.agent.providers.codex.model.trim();
                if m.is_empty() {
                    None
                } else {
                    Some(m.to_string())
                }
            },
            {
                let m = cfg.agent.providers.opencode.model.trim();
                if m.is_empty() {
                    None
                } else {
                    Some(m.to_string())
                }
            },
            cfg.agent.improve_timeout_secs,
        )
    };

    // Read the persisted description session ID for this workflow (if any).
    let resume_id: Option<String> = {
        let wf_arc = engine.engine.workflows_arc();
        let wf = wf_arc.read().await;
        wf.get(ticket_key)
            .and_then(|w| w.description_session_id.clone())
    };

    let worktree = std::env::temp_dir();
    let cancel = CancellationToken::new();

    // "Improve with AI" / "Prompt ticket" runs the agent CLI **directly in the
    // main container** — no DinD worker. Per-invocation credential isolation:
    // inject the caller's own provider API key as a per-process env var when they
    // have one; otherwise the CLI inherits the main container's ambient
    // (app-instance) credential. Two concurrent improves stay isolated because
    // each passes its own env to its own child process — no global env mutation.
    let container_runner: Option<ContainerRunner> = None;
    let cred_env: Option<(String, String)> =
        resolve_user_credential_env(auth_state, provider, user_id).await;
    let extra_env: Vec<(&str, &str)> = cred_env
        .as_ref()
        .map(|(k, v)| vec![(k.as_str(), v.as_str())])
        .unwrap_or_default();

    // Run with the configured provider.
    let (session_id, output) = match provider {
        AiAgentProvider::Claude => {
            let sess = ClaudeSession::run_prompt(
                &worktree,
                prompt,
                cancel,
                improve_timeout,
                None, // line_tx
                model.as_deref(),
                resume_id.as_deref(),
                container_runner.as_ref(),
                // System prompt is only effective on a fresh session; on resume the existing
                // session already has its system prompt — inject it into the user message instead.
                if resume_id.is_some() {
                    None
                } else {
                    system_prompt
                },
                &extra_env,
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
                improve_timeout,
                None, // line_tx
                if effective_model == "Auto" {
                    None
                } else {
                    Some(effective_model)
                },
                resume_id.as_deref(),
                container_runner.as_ref(),
                0, // no idle nudge: improve runs non-streaming, bounded by improve_timeout
                &extra_env,
            )
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            (sess.session_id, sess.output)
        }
        AiAgentProvider::Codex => {
            let sess = CodexSession::run_prompt(
                &worktree,
                prompt,
                cancel,
                improve_timeout,
                None, // line_tx
                codex_model.as_deref(),
                resume_id.as_deref(),
                container_runner.as_ref(),
                &extra_env,
            )
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            (sess.session_id, sess.output)
        }
        AiAgentProvider::OpenCode => {
            let sess = OpenCodeSession::run_prompt(
                &worktree,
                prompt,
                cancel,
                improve_timeout,
                None, // line_tx
                opencode_model.as_deref(),
                resume_id.as_deref(),
                container_runner.as_ref(),
                &extra_env,
            )
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            (sess.session_id, sess.output)
        }
    };

    // Persist the session ID back to the workflow so the next call resumes it.
    {
        let wf_arc = engine.engine.workflows_arc();
        let mut wf = wf_arc.write().await;
        if let Some(w) = wf.get_mut(ticket_key) {
            w.description_session_id = Some(session_id);
        }
    }
    // Best-effort snapshot so the session survives a restart.
    let _ = engine.engine.sync_workflow_snapshot().await;

    Ok(output)
}

/// True when `ticket_key` is a live work item (DB row or in-memory map),
/// regardless of owner. Used to decide whether the description-editing
/// endpoints must enforce ownership.
async fn workflow_exists(engine: &EngineState, auth_state: &AuthState, ticket_key: &str) -> bool {
    if let Some(db) = auth_state.db.as_ref()
        && let Ok(Some(_)) =
            takuto_core::db::work_items::get_access_fields_by_ticket_key(db.adapter(), ticket_key)
                .await
    {
        return true;
    }
    engine
        .engine
        .workflows_arc()
        .read()
        .await
        .contains_key(ticket_key)
}

/// Authorize an improve/prompt-description request. "Improve with AI" is offered
/// both for live dashboard items **and** in the Add-to-Dashboard preview flow
/// (where the ticket is not a work item yet). When the ticket IS a live work
/// item, only its owner may touch it; when it isn't on any board, any
/// authenticated caller may edit the preview description.
async fn authorize_description_session(
    engine: &EngineState,
    auth_state: &AuthState,
    auth: &AuthenticatedUser,
    ticket_key: &str,
) -> Result<(), (StatusCode, String)> {
    if workflow_exists(engine, auth_state, ticket_key).await {
        require_workflow_access(engine, auth_state, auth, ticket_key)
            .await
            .map_err(|s| (s, String::new()))
    } else {
        Ok(())
    }
}

/// `POST /api/tickets/{key}/improve` — run a headless Claude session to improve the ticket description.
pub async fn improve_ticket(
    State(engine): State<EngineState>,
    State(auth_state): State<AuthState>,
    State(cfg_state): State<ConfigState>,
    Path(key): Path<String>,
    Extension(auth): Extension<AuthenticatedUser>,
    Json(body): Json<ImproveTicketBody>,
) -> Result<Json<ImproveTicketResponse>, (StatusCode, String)> {
    // Owner-only for live items; allowed for not-yet-added tickets (the
    // Add-to-Dashboard preview flow has no work item yet).
    authorize_description_session(&engine, &auth_state, &auth, &key).await?;
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
    if let Some(extra) = &body.prompt
        && !extra.trim().is_empty()
    {
        prompt.push_str(&format!("\n\n**Additional instructions:** {extra}"));
    }

    let output = run_description_session(
        &engine,
        &auth_state,
        &cfg_state,
        &key,
        &auth.user_id,
        &prompt,
        Some(IMPROVE_SYSTEM_PROMPT),
    )
    .await?;

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
    State(engine): State<EngineState>,
    State(auth_state): State<AuthState>,
    State(cfg_state): State<ConfigState>,
    Path(key): Path<String>,
    Extension(auth): Extension<AuthenticatedUser>,
    Json(body): Json<PromptTicketBody>,
) -> Result<Json<PromptTicketResponse>, (StatusCode, String)> {
    // Owner-only for live items; allowed for not-yet-added tickets (preview flow).
    authorize_description_session(&engine, &auth_state, &auth, &key).await?;
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

    let output = run_description_session(
        &engine,
        &auth_state,
        &cfg_state,
        &key,
        &auth.user_id,
        &prompt,
        Some(PROMPT_SYSTEM_PROMPT),
    )
    .await?;

    Ok(Json(PromptTicketResponse { response: output }))
}

/// `POST /api/tickets/{key}/update-description` — persist the improved description to the ticketing system.
pub async fn update_ticket_description(
    State(engine): State<EngineState>,
    State(auth_state): State<AuthState>,
    State(cfg_state): State<ConfigState>,
    Path(key): Path<String>,
    Extension(auth): Extension<AuthenticatedUser>,
    Json(body): Json<UpdateDescriptionBody>,
) -> Result<Json<serde_json::Value>, JiraRouteError> {
    // Owner-only for live items; allowed for not-yet-added tickets (the
    // Add-to-Dashboard preview flow saves the edited description before the item
    // is on the board).
    authorize_description_session(&engine, &auth_state, &auth, &key).await?;
    // Resolve the cwd for `gh` / `acli` from the work item's repository_id, or —
    // for a not-yet-added ticket — from the caller-supplied `repository` name;
    // falls back to the global repo path.
    let workflow_repo_path = resolve_workflow_repo_path(
        &engine,
        &auth_state,
        &cfg_state,
        &key,
        body.repository.as_deref(),
    )
    .await;
    match cfg_state.ticketing_system {
        TicketingSystem::None => {
            // No external ticketing system — persist to the in-memory work item.
            // Unlike GitHub/Jira, a None-mode ticket has no existence outside the
            // dashboard, so there is no preview flow: a missing work item is a
            // 404 (and avoids leaking existence — see the IDOR fix).
            let wf_arc = engine.engine.workflows_arc();
            let mut workflows = wf_arc.write().await;
            let Some(wf) = workflows.get_mut(&key) else {
                return Err((StatusCode::NOT_FOUND, String::new()).into());
            };
            wf.ticket_description = body.description.clone();
            if let Some(ref s) = body.summary {
                wf.ticket_summary = s.clone();
            }
            drop(workflows);
            // Best-effort snapshot sync so the edit survives a restart.
            let _ = engine.engine.sync_workflow_snapshot().await;
            Ok(Json(serde_json::json!({})))
        }
        TicketingSystem::GitHub => {
            let remote = {
                let config = cfg_state.config.read().await;
                config.git.remote.clone()
            };
            let repo_path = workflow_repo_path.clone();
            let remote_url = takuto_core::git::remote::resolve_remote_url(&repo_path, &remote)
                .await
                .map_err(|e| {
                    (
                        StatusCode::BAD_REQUEST,
                        format!("Cannot resolve git remote URL: {e}"),
                    )
                })?;
            let owner_repo = parse_github_repo(&remote_url).ok_or_else(|| {
                (
                    StatusCode::BAD_REQUEST,
                    format!("Cannot parse GitHub owner/repo from git remote URL: {remote_url:?}"),
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

            // Prefer the GitHub App installation token; on PAT-only deployments
            // fall back to the caller's per-user PAT so `gh api` authenticates.
            let app_token = engine
                .engine
                .actions()
                .get_gh_installation_token(&repo_path)
                .await;
            let gh_token = takuto_core::github::github_token_app_then_pat(
                app_token,
                auth_state.git_auth_resolver.as_ref(),
                Some(&auth.user_id),
                takuto_core::github::auth_resolver::GitAction::IssueComment,
            )
            .await;

            let endpoint = format!("repos/{owner_repo}/issues/{issue_number}");
            let body_field = format!("body={}", body.description);
            let mut gh_args: Vec<&str> = vec![
                "api",
                "--method",
                "PATCH",
                &endpoint,
                "--raw-field",
                &body_field,
            ];
            let title_field;
            if let Some(ref s) = body.summary {
                title_field = format!("title={s}");
                gh_args.push("--raw-field");
                gh_args.push(&title_field);
            }
            let gh_token_ref: &str = gh_token.as_deref().unwrap_or("");
            let extra_env: &[(&str, &str)] = if gh_token.is_some() {
                &[("GH_TOKEN", gh_token_ref)]
            } else {
                &[]
            };
            let output = takuto_core::process::run_command_with_env(
                "gh",
                &gh_args,
                &repo_path,
                tokio_util::sync::CancellationToken::new(),
                extra_env,
            )
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

            if !output.success() {
                return Err((
                    StatusCode::BAD_GATEWAY,
                    format!(
                        "gh api PATCH issues/{issue_number} failed: {}",
                        output.stderr.trim()
                    ),
                )
                    .into());
            }

            // Update in-memory description (and optionally summary) so the next
            // `GET /api/workflows` returns the freshly saved value — prevents the
            // dashboard from showing stale text when the user reopens the modal.
            {
                let wf_arc = engine.engine.workflows_arc();
                let mut workflows = wf_arc.write().await;
                if let Some(wf) = workflows.get_mut(&key) {
                    wf.ticket_description = body.description.clone();
                    if let Some(ref s) = body.summary {
                        wf.ticket_summary = s.clone();
                    }
                }
                drop(workflows);
                let _ = engine.engine.sync_workflow_snapshot().await;
            }

            Ok(Json(serde_json::json!({})))
        }
        TicketingSystem::Jira => {
            // Prefer the caller's per-user Jira REST credential (the editor is
            // owner-gated above, so the caller IS the owner); fall back to acli.
            // A REST 401/403 surfaces as the `jira_credential_invalid` modal code.
            let rest_cred = match auth_state.db.as_ref() {
                Some(db) => resolve_rest_credential(db, &auth.user_id).await,
                None => None,
            };
            match rest_cred {
                Some(cred) => {
                    JiraRestClient::new(auth_state.jira_http.clone(), cred)
                        .update_description(&key, &body.description)
                        .await
                }
                None => {
                    JiraClient::new(workflow_repo_path.clone())
                        .update_description(&key, &body.description)
                        .await
                }
            }
            .map_err(|e| map_jira_err(e, StatusCode::BAD_GATEWAY))?;

            // Update in-memory description (and optionally summary) so the next
            // `GET /api/workflows` returns the freshly saved value — prevents the
            // dashboard from showing stale text when the user reopens the modal.
            {
                let wf_arc = engine.engine.workflows_arc();
                let mut workflows = wf_arc.write().await;
                if let Some(wf) = workflows.get_mut(&key) {
                    wf.ticket_description = body.description.clone();
                    if let Some(ref s) = body.summary {
                        wf.ticket_summary = s.clone();
                    }
                }
                drop(workflows);
                let _ = engine.engine.sync_workflow_snapshot().await;
            }

            Ok(Json(serde_json::json!({})))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use tokio::sync::RwLock;

    use takuto_core::actions::dry_run::DryRunActions;
    use takuto_core::config::{Config, TicketingSystem};
    use takuto_core::workflow::engine::{Workflow, WorkflowEngine};

    use crate::state::AppState;

    /// Build a minimal `AppState` for testing `update_ticket_description` in `None` mode.
    fn test_app_state_none() -> AppState {
        let config = Arc::new(RwLock::new(Config::default()));
        let actions: Arc<dyn takuto_core::actions::traits::ExternalActions> =
            Arc::new(DryRunActions::new("origin".to_string(), None));
        let jira_available = Arc::new(AtomicBool::new(false));
        let engine = Arc::new(WorkflowEngine::new(
            config.clone(),
            actions,
            1,
            jira_available.clone(),
            TicketingSystem::None,
            std::env::temp_dir(),
        ));
        use crate::state::{AuthState, ConfigState, EditorState, EngineState, RunCommandState};
        AppState::new(
            EngineState {
                engine,
                polling_paused: Arc::new(AtomicBool::new(false)),
                clone_in_progress: Arc::new(AtomicBool::new(false)),
                system_status: Arc::new(RwLock::new(
                    takuto_core::docker_hooks::SystemStatus::default(),
                )),
            },
            AuthState {
                db: None,
                gh_client: Arc::new(takuto_core::auth::RealGhClient::new()),
                git_auth_resolver: None,
                jira_http: Arc::new(takuto_core::jira::RealJiraHttp::new()),
            },
            ConfigState {
                config,
                config_path: std::env::temp_dir().join("config.toml"),
                config_writer: None,
                ticketing_system: TicketingSystem::None,
                jira_available,
                preflight_error: None,
                work_item_flow_defaults: std::sync::Arc::new(Vec::new()),
            },
            EditorState {
                editor_scanners: Arc::new(RwLock::new(HashMap::new())),
                dynamic_forwards: Arc::new(RwLock::new(HashMap::new())),
                terminal_ports: Arc::new(RwLock::new(HashMap::new())),
                editor_bundles: Arc::new(RwLock::new(HashMap::new())),
                path_token_registry: crate::session_registry::PathTokenRegistry::new(),
            },
            RunCommandState {
                run_commands: Arc::new(RwLock::new(HashMap::new())),
                run_command_bundles: Arc::new(RwLock::new(HashMap::new())),
                spawner: Arc::new(crate::test_helpers::FakeSpawner::ready()),
            },
        )
    }

    /// Inserts a workflow owned by the given user id into the engine.
    async fn insert_workflow(engine: &WorkflowEngine, key: &str, summary: &str, description: &str) {
        let mut wf = Workflow::new(
            key.to_string(),
            summary.to_string(),
            false,
            false,
            TicketingSystem::None,
            None,
            "test-workspace".into(),
        );
        wf.ticket_description = description.to_string();
        // Tests below pass a matching AuthenticatedUser; tag the workflow with
        // the same id so `require_workflow_access` passes.
        wf.user_id = Some("test-user".to_string());
        engine
            .workflows_arc()
            .write()
            .await
            .insert(key.to_string(), wf);
    }

    /// Build an `AuthenticatedUser` matching the workflow owner used by
    /// `insert_workflow`. Tests need this because handlers now require the
    /// `Extension<AuthenticatedUser>` extractor (IDOR-safe).
    fn test_auth() -> AuthenticatedUser {
        AuthenticatedUser {
            user_id: "test-user".to_string(),
            role: takuto_core::db::models::UserRole::User,
        }
    }

    #[test]
    fn provider_api_key_env_maps_env_injectable_providers() {
        assert_eq!(
            provider_api_key_env(AiAgentProvider::Claude),
            Some("CLAUDE_CODE_OAUTH_TOKEN")
        );
        assert_eq!(
            provider_api_key_env(AiAgentProvider::Cursor),
            Some("CURSOR_API_KEY")
        );
        assert_eq!(
            provider_api_key_env(AiAgentProvider::Codex),
            Some("OPENAI_API_KEY")
        );
        // OpenCode reads a config file, not an env var.
        assert_eq!(provider_api_key_env(AiAgentProvider::OpenCode), None);
    }

    #[tokio::test]
    async fn resolve_user_credential_env_none_without_db() {
        // No DB → no per-user credential resolvable → the CLI inherits the main
        // container's ambient (app-instance) credential.
        let state = test_app_state_none();
        let got =
            resolve_user_credential_env(&state.auth, AiAgentProvider::Cursor, "test-user").await;
        assert!(got.is_none());
    }

    #[tokio::test]
    async fn improve_allowed_for_ticket_not_on_any_board() {
        // The Add-to-Dashboard preview flow improves a ticket that isn't a work
        // item yet — authorization must NOT 404 it.
        let state = test_app_state_none();
        assert!(
            !workflow_exists(&state.engine, &state.auth, "GH-NEVER-ADDED").await,
            "ticket has no work item"
        );
        let res = authorize_description_session(
            &state.engine,
            &state.auth,
            &test_auth(),
            "GH-NEVER-ADDED",
        )
        .await;
        assert!(
            res.is_ok(),
            "preview-flow improve must be allowed, got {res:?}"
        );
    }

    /// Saving a description (without summary) updates `ticket_description` in memory.
    #[tokio::test]
    async fn update_description_none_mode_updates_description() {
        let state = test_app_state_none();
        insert_workflow(
            &state.engine.engine,
            "T-1",
            "Old Summary",
            "Old description",
        )
        .await;

        let result = update_ticket_description(
            State(state.engine.clone()),
            State(state.auth.clone()),
            State(state.config.clone()),
            Path("T-1".to_string()),
            Extension(test_auth()),
            Json(UpdateDescriptionBody {
                description: "New description".to_string(),
                summary: None,
                repository: None,
            }),
        )
        .await;

        assert!(result.is_ok());

        let wf_arc = state.engine.engine.workflows_arc();
        let workflows = wf_arc.read().await;
        let wf = workflows.get("T-1").expect("workflow should exist");
        assert_eq!(wf.ticket_description, "New description");
        // Summary should remain unchanged when not provided.
        assert_eq!(wf.ticket_summary, "Old Summary");
    }

    /// Saving a description and summary updates both in memory.
    #[tokio::test]
    async fn update_description_none_mode_updates_both() {
        let state = test_app_state_none();
        insert_workflow(
            &state.engine.engine,
            "T-2",
            "Old Summary",
            "Old description",
        )
        .await;

        let result = update_ticket_description(
            State(state.engine.clone()),
            State(state.auth.clone()),
            State(state.config.clone()),
            Path("T-2".to_string()),
            Extension(test_auth()),
            Json(UpdateDescriptionBody {
                description: "New description".to_string(),
                summary: Some("New Summary".to_string()),
                repository: None,
            }),
        )
        .await;

        assert!(result.is_ok());

        let wf_arc = state.engine.engine.workflows_arc();
        let workflows = wf_arc.read().await;
        let wf = workflows.get("T-2").expect("workflow should exist");
        assert_eq!(wf.ticket_description, "New description");
        assert_eq!(wf.ticket_summary, "New Summary");
    }

    /// Saving a description for a non-existent workflow returns 404.
    /// The previous behaviour was a silent no-op success; after the IDOR fix the
    /// access guard rejects unknown keys identically to "wrong owner" so existence
    /// is not leaked across users.
    #[tokio::test]
    async fn update_description_none_mode_missing_workflow() {
        let state = test_app_state_none();
        // No workflow inserted — the key "T-3" does not exist.

        let result = update_ticket_description(
            State(state.engine.clone()),
            State(state.auth.clone()),
            State(state.config.clone()),
            Path("T-3".to_string()),
            Extension(test_auth()),
            Json(UpdateDescriptionBody {
                description: "Some description".to_string(),
                summary: None,
                repository: None,
            }),
        )
        .await;

        let err = result.expect_err("missing workflow must be 404");
        assert!(
            matches!(err, JiraRouteError::Plain(StatusCode::NOT_FOUND, _)),
            "missing workflow must map to a plain 404"
        );
    }
}
