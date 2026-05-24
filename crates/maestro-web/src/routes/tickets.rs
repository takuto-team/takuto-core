// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::path::PathBuf;

use axum::Json;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use maestro_core::claude::session::ClaudeSession;
use maestro_core::codex::CodexSession;
use maestro_core::config::{AiAgentProvider, TicketingSystem, cursor_model_for_cli};
use maestro_core::container::ContainerRunner;
use maestro_core::cursor::session::CursorSession;
use maestro_core::opencode::OpenCodeSession;
use maestro_core::jira::client::JiraClient;

use crate::auth::AuthenticatedUser;
use crate::routes::github::parse_github_repo;
use crate::routes::workflows::require_workflow_access;
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

/// Plan-10: resolve the on-disk `local_path` of the repository associated with
/// the workflow keyed by `ticket_key`. Falls back to `cfg.git.repo_path` when
/// the workflow has no `repository_id` (legacy snapshots) or when no DB is
/// attached (test paths).
async fn resolve_workflow_repo_path(state: &AppState, ticket_key: &str) -> PathBuf {
    let (repo_id, ws_name) = {
        let wf_arc = state.engine.engine.workflows_arc();
        let wf = wf_arc.read().await;
        wf.get(ticket_key)
            .map(|w| (w.repository_id.clone(), w.workspace_name.clone()))
            .unwrap_or_default()
    };
    if let Some(database) = state.auth.db.as_ref() {
        let conn = database.conn().lock().await;
        if let Some(id) = repo_id.as_deref()
            && let Ok(Some(row)) = maestro_core::db::repositories::get(&conn, id)
        {
            return PathBuf::from(&row.local_path);
        }
        if !ws_name.is_empty()
            && let Ok(Some(row)) = maestro_core::db::repositories::get_by_name(&conn, &ws_name)
        {
            return PathBuf::from(&row.local_path);
        }
    }
    let cfg = state.config.config.read().await;
    PathBuf::from(&cfg.git.repo_path)
}

/// Run a description-editing AI session (improve / prompt) using the configured provider.
///
/// Reads the workflow's `description_session_id` to resume the shared conversation, then
/// writes the new session ID back so the next call continues in the same context.
/// Falls back gracefully when the ticket has no associated workflow in the map.
async fn run_description_session(
    state: &AppState,
    ticket_key: &str,
    user_id: &str,
    prompt: &str,
    system_prompt: Option<&str>,
) -> Result<String, (StatusCode, String)> {
    // Snapshot the config fields we need before any await point.
    let (
        provider,
        model,
        cursor_cli,
        cursor_model,
        codex_model,
        opencode_model,
        improve_timeout,
        worker_image,
    ) = {
        let cfg = state.config.config.read().await;
        (
            cfg.agent.provider,
            // Task #44: route only feeds `model` into the Claude branch
            // (`AiAgentProvider::Claude` → `ClaudeSession::run_prompt`),
            // so resolve via the sub-table-aware helper. Previously
            // sourced from the legacy `cfg.agent.model` directly, which
            // ignored an empty sub-table value and forced a stale
            // migrated model on every improve/prompt invocation.
            cfg.agent.effective_claude_model().map(str::to_string),
            cfg.agent.cursor_cli.clone(),
            cfg.agent.cursor_model.clone(),
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
            cfg.general.worker_image.clone(),
        )
    };

    // Read the persisted description session ID for this workflow (if any).
    let resume_id: Option<String> = {
        let wf_arc = state.engine.engine.workflows_arc();
        let wf = wf_arc.read().await;
        wf.get(ticket_key)
            .and_then(|w| w.description_session_id.clone())
    };

    let worktree = std::env::temp_dir();
    let cancel = CancellationToken::new();

    // Build an ephemeral container runner so the AI agent never runs in the main container.
    // Falls back to None only when DinD is genuinely unavailable (dev/test env).
    let container_runner: Option<ContainerRunner> = if ContainerRunner::is_available() {
        let image = if worker_image.is_empty() {
            ContainerRunner::discover_worker_image()
                .await
                .unwrap_or_else(|| "maestro:latest".to_string())
        } else {
            worker_image
        };
        // Note: no .with_isolate_workspace() here — the improve session uses a
        // temp directory, not a real worktree under /workspace/worktrees/, so the
        // per-issue isolation logic (which derives the repo root from the grandparent)
        // does not apply.  The session is already sandboxed in its own ephemeral
        // container; it does not need worktree-level isolation.
        let mut runner =
            ContainerRunner::new(&format!("improve-{ticket_key}"), &worktree, &image);

        // Phase 2b.3.x: attach a per-request `WorkerSecretsBundle` so the
        // ephemeral worker reads the caller's per-user provider key + GitHub
        // token from tmpfs files instead of `docker run -e`. Falls back to
        // the legacy `PASSTHROUGH_ENV` path when:
        //   - master key unavailable (degraded mode), OR
        //   - user has no credential AND active provider's
        //     `allow_shared_default = false`.
        // The first case surfaces 503; the second surfaces 503 + a
        // structured `credential_required` error so the dashboard can
        // prompt the user to paste an API key.
        if let Some(ref resolver) = state.auth.git_auth_resolver
            && let Some(db) = state.auth.db.as_ref()
        {
            if db.master_key().is_none() {
                return Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    "master_key_unavailable".into(),
                ));
            }
            let cfg_snapshot = state.config.config.read().await.clone();
            match maestro_core::auth::bundle::build_for_endpoint(
                &cfg_snapshot,
                db,
                resolver,
                user_id,
            )
            .await
            {
                Ok(bundle) => {
                    tracing::info!(
                        action = "improve_ticket",
                        user_id = %user_id,
                        ticket = %ticket_key,
                        source = "user_credential",
                        "Attached WorkerSecretsBundle to improve/prompt runner"
                    );
                    runner = runner.with_secrets_bundle(std::sync::Arc::new(bundle));
                }
                Err(e) => {
                    let msg = e.to_string();
                    if msg.contains("provider_credential_missing") {
                        return Err((
                            StatusCode::SERVICE_UNAVAILABLE,
                            serde_json::json!({
                                "error": "credential_required",
                                "provider": cfg_snapshot.agent.provider.as_str(),
                            })
                            .to_string(),
                        ));
                    }
                    tracing::warn!(
                        ticket = %ticket_key,
                        error = %e,
                        "Bundle build failed for improve/prompt — falling back to legacy passthrough"
                    );
                }
            }
        }

        Some(runner)
    } else {
        tracing::warn!(
            ticket = %ticket_key,
            "DinD not available — running improve/prompt session directly in main container"
        );
        None
    };

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
            )
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            (sess.session_id, sess.output)
        }
    };

    // Persist the session ID back to the workflow so the next call resumes it.
    {
        let wf_arc = state.engine.engine.workflows_arc();
        let mut wf = wf_arc.write().await;
        if let Some(w) = wf.get_mut(ticket_key) {
            w.description_session_id = Some(session_id);
        }
    }
    // Best-effort snapshot so the session survives a restart.
    let _ = state.engine.engine.sync_workflow_snapshot().await;

    Ok(output)
}

/// `POST /api/tickets/{key}/improve` — run a headless Claude session to improve the ticket description.
pub async fn improve_ticket(
    State(state): State<AppState>,
    Path(key): Path<String>,
    Extension(auth): Extension<AuthenticatedUser>,
    Json(body): Json<ImproveTicketBody>,
) -> Result<Json<ImproveTicketResponse>, (StatusCode, String)> {
    // AC-2: only the workflow owner may invoke this. The helper returns
    // `NOT_FOUND` for both "missing" and "wrong owner" so existence is not
    // leaked across users.
    require_workflow_access(&state, &auth, &key)
        .await
        .map_err(|s| (s, String::new()))?;
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

    let output =
        run_description_session(&state, &key, &auth.user_id, &prompt, Some(IMPROVE_SYSTEM_PROMPT))
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
    State(state): State<AppState>,
    Path(key): Path<String>,
    Extension(auth): Extension<AuthenticatedUser>,
    Json(body): Json<PromptTicketBody>,
) -> Result<Json<PromptTicketResponse>, (StatusCode, String)> {
    // AC-2: only the workflow owner may invoke this.
    require_workflow_access(&state, &auth, &key)
        .await
        .map_err(|s| (s, String::new()))?;
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
        run_description_session(&state, &key, &auth.user_id, &prompt, Some(PROMPT_SYSTEM_PROMPT))
            .await?;

    Ok(Json(PromptTicketResponse { response: output }))
}

/// `POST /api/tickets/{key}/update-description` — persist the improved description to the ticketing system.
pub async fn update_ticket_description(
    State(state): State<AppState>,
    Path(key): Path<String>,
    Extension(auth): Extension<AuthenticatedUser>,
    Json(body): Json<UpdateDescriptionBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // AC-2: only the workflow owner may invoke this.
    require_workflow_access(&state, &auth, &key)
        .await
        .map_err(|s| (s, String::new()))?;
    // Plan-10: resolve the cwd for `gh` / `acli` from the workflow's
    // repository_id rather than the global cfg.git.repo_path.
    let workflow_repo_path = resolve_workflow_repo_path(&state, &key).await;
    match state.config.ticketing_system {
        TicketingSystem::None => {
            // No external ticketing system — persist to the in-memory workflow.
            let wf_arc = state.engine.engine.workflows_arc();
            let mut workflows = wf_arc.write().await;
            if let Some(wf) = workflows.get_mut(&key) {
                wf.ticket_description = body.description.clone();
                if let Some(ref s) = body.summary {
                    wf.ticket_summary = s.clone();
                }
            }
            drop(workflows);
            // Best-effort snapshot sync so the edit survives a restart.
            let _ = state.engine.engine.sync_workflow_snapshot().await;
            Ok(Json(serde_json::json!({})))
        }
        TicketingSystem::GitHub => {
            let remote = {
                let config = state.config.config.read().await;
                config.git.remote.clone()
            };
            let repo_path = workflow_repo_path.clone();
            let remote_url = maestro_core::git::remote::resolve_remote_url(&repo_path, &remote)
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

            // Inject GitHub App installation token when configured so `gh api`
            // works without personal user authentication.
            let gh_token = state
                .engine
                .engine
                .actions()
                .get_gh_installation_token(&repo_path)
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
            let output = maestro_core::process::run_command_with_env(
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
                ));
            }

            // Update in-memory description (and optionally summary) so the next
            // `GET /api/workflows` returns the freshly saved value — prevents the
            // dashboard from showing stale text when the user reopens the modal.
            {
                let wf_arc = state.engine.engine.workflows_arc();
                let mut workflows = wf_arc.write().await;
                if let Some(wf) = workflows.get_mut(&key) {
                    wf.ticket_description = body.description.clone();
                    if let Some(ref s) = body.summary {
                        wf.ticket_summary = s.clone();
                    }
                }
                drop(workflows);
                let _ = state.engine.engine.sync_workflow_snapshot().await;
            }

            Ok(Json(serde_json::json!({})))
        }
        TicketingSystem::Jira => {
            let client = JiraClient::new(workflow_repo_path.clone());
            client
                .update_description(&key, &body.description)
                .await
                .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;

            // Update in-memory description (and optionally summary) so the next
            // `GET /api/workflows` returns the freshly saved value — prevents the
            // dashboard from showing stale text when the user reopens the modal.
            {
                let wf_arc = state.engine.engine.workflows_arc();
                let mut workflows = wf_arc.write().await;
                if let Some(wf) = workflows.get_mut(&key) {
                    wf.ticket_description = body.description.clone();
                    if let Some(ref s) = body.summary {
                        wf.ticket_summary = s.clone();
                    }
                }
                drop(workflows);
                let _ = state.engine.engine.sync_workflow_snapshot().await;
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

    use maestro_core::actions::dry_run::DryRunActions;
    use maestro_core::config::{Config, TicketingSystem};
    use maestro_core::workflow::engine::{Workflow, WorkflowEngine};

    /// Build a minimal `AppState` for testing `update_ticket_description` in `None` mode.
    fn test_app_state_none() -> AppState {
        let config = Arc::new(RwLock::new(Config::default()));
        let actions: Arc<dyn maestro_core::actions::traits::ExternalActions> = Arc::new(
            DryRunActions::new("origin".to_string(), None),
        );
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
                    maestro_core::docker_hooks::SystemStatus::default(),
                )),
            },
            AuthState {
                db: None,
                gh_client: Arc::new(maestro_core::auth::RealGhClient::new()),
                git_auth_resolver: None,
            },
            ConfigState {
                config,
                config_path: std::env::temp_dir().join("config.toml"),
                config_writer: None,
                ticketing_system: TicketingSystem::None,
                jira_available,
                preflight_error: None,
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
        // the same id so `require_workflow_access` passes (AC-2).
        wf.user_id = Some("test-user".to_string());
        engine
            .workflows_arc()
            .write()
            .await
            .insert(key.to_string(), wf);
    }

    /// Build an `AuthenticatedUser` matching the workflow owner used by
    /// `insert_workflow`. Tests need this because handlers now require the
    /// `Extension<AuthenticatedUser>` extractor (AC-2 IDOR fix).
    fn test_auth() -> AuthenticatedUser {
        AuthenticatedUser {
            user_id: "test-user".to_string(),
            role: maestro_core::db::models::UserRole::User,
        }
    }

    /// Saving a description (without summary) updates `ticket_description` in memory.
    #[tokio::test]
    async fn update_description_none_mode_updates_description() {
        let state = test_app_state_none();
        insert_workflow(&state.engine.engine, "T-1", "Old Summary", "Old description").await;

        let result = update_ticket_description(
            State(state.clone()),
            Path("T-1".to_string()),
            Extension(test_auth()),
            Json(UpdateDescriptionBody {
                description: "New description".to_string(),
                summary: None,
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
        insert_workflow(&state.engine.engine, "T-2", "Old Summary", "Old description").await;

        let result = update_ticket_description(
            State(state.clone()),
            Path("T-2".to_string()),
            Extension(test_auth()),
            Json(UpdateDescriptionBody {
                description: "New description".to_string(),
                summary: Some("New Summary".to_string()),
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

    /// Saving a description for a non-existent workflow returns 404 (AC-2 G/W/T 2.5).
    /// The previous behaviour was a silent no-op success; after the IDOR fix the
    /// access guard rejects unknown keys identically to "wrong owner" so existence
    /// is not leaked across users.
    #[tokio::test]
    async fn update_description_none_mode_missing_workflow() {
        let state = test_app_state_none();
        // No workflow inserted — the key "T-3" does not exist.

        let result = update_ticket_description(
            State(state.clone()),
            Path("T-3".to_string()),
            Extension(test_auth()),
            Json(UpdateDescriptionBody {
                description: "Some description".to_string(),
                summary: None,
            }),
        )
        .await;

        let err = result.expect_err("missing workflow must be 404");
        assert_eq!(err.0, StatusCode::NOT_FOUND);
    }
}
