// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use std::sync::atomic::Ordering;
use tracing::{error, info};

use crate::state::AppState;

/// Drop guard that resets `clone_in_progress` to `false` when the async clone
/// task finishes — whether it completes normally or panics.
struct CloneGuard(std::sync::Arc<std::sync::atomic::AtomicBool>);
impl Drop for CloneGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

#[derive(Serialize)]
pub struct GitHubRepoRow {
    pub full_name: String,
    pub description: String,
    pub private: bool,
    pub html_url: String,
}

#[derive(Deserialize)]
pub struct RepoListQuery {
    #[serde(default)]
    pub q: String,
}

/// `GET /api/github/repos` — list GitHub repos accessible by the authenticated user.
pub async fn list_github_repos(
    State(state): State<AppState>,
    Query(query): Query<RepoListQuery>,
) -> Result<Json<Vec<GitHubRepoRow>>, (StatusCode, String)> {
    let repo_path = {
        let config = state.config.read().await;
        std::path::PathBuf::from(&config.git.repo_path)
    };

    let gh_token = state
        .engine
        .actions
        .get_gh_installation_token(&repo_path)
        .await;

    let env: Vec<(&str, &str)> = gh_token
        .as_deref()
        .map(|t| vec![("GH_TOKEN", t)])
        .unwrap_or_default();

    // If a search query is provided, use the search endpoint; otherwise list user repos.
    let search_query;
    let args: Vec<String> = if !query.q.is_empty() {
        search_query = format!("{} in:name", query.q);
        vec![
            "api".to_string(),
            "search/repositories".to_string(),
            "--method".to_string(),
            "GET".to_string(),
            "--field".to_string(),
            format!("q={search_query}"),
            "--field".to_string(),
            "per_page=50".to_string(),
        ]
    } else {
        vec![
            "api".to_string(),
            "user/repos".to_string(),
            "--method".to_string(),
            "GET".to_string(),
            "--field".to_string(),
            "per_page=100".to_string(),
            "--field".to_string(),
            "sort=updated".to_string(),
            "--field".to_string(),
            "type=all".to_string(),
        ]
    };

    let args_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

    let output = maestro_core::process::run_command_with_env(
        "gh",
        &args_refs,
        &repo_path,
        tokio_util::sync::CancellationToken::new(),
        &env,
    )
    .await
    .map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            format!("Failed to list GitHub repos: {e}"),
        )
    })?;

    if !output.success() {
        return Err((
            StatusCode::BAD_GATEWAY,
            format!(
                "GitHub API error: {}",
                maestro_core::github::gh_api_error_message(output.stderr.trim(), "Metadata: Read")
            ),
        ));
    }

    let json: serde_json::Value = serde_json::from_str(output.stdout.trim()).map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            format!("Failed to parse GitHub response: {e}"),
        )
    })?;

    // Handle both /user/repos (returns array) and /search/repositories (returns { items: [...] })
    let items = if let Some(arr) = json.as_array() {
        arr.clone()
    } else if let Some(items) = json.get("items").and_then(|v| v.as_array()) {
        items.clone()
    } else {
        Vec::new()
    };

    let repos: Vec<GitHubRepoRow> = items
        .iter()
        .filter_map(|v| {
            Some(GitHubRepoRow {
                full_name: v.get("full_name")?.as_str()?.to_string(),
                description: v
                    .get("description")
                    .and_then(|d| d.as_str())
                    .unwrap_or("")
                    .to_string(),
                private: v.get("private").and_then(|p| p.as_bool()).unwrap_or(false),
                html_url: v.get("html_url")?.as_str()?.to_string(),
            })
        })
        .collect();

    Ok(Json(repos))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CloneRepoBody {
    pub full_name: String,
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Serialize)]
pub struct CloneRepoResponse {
    pub status: String,
}

/// `POST /api/repos/clone` — Clone a GitHub repo into the configured repo_path.
pub async fn clone_repo(
    State(state): State<AppState>,
    Json(body): Json<CloneRepoBody>,
) -> Result<(StatusCode, Json<CloneRepoResponse>), (StatusCode, String)> {
    // Validate full_name format: must be "owner/repo" with only safe characters.
    let valid = {
        let parts: Vec<&str> = body.full_name.split('/').collect();
        parts.len() == 2
            && parts.iter().all(|p| {
                !p.is_empty()
                    && p.chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
                    && !p.starts_with('.')
            })
    };
    if !valid {
        return Err((
            StatusCode::BAD_REQUEST,
            "Invalid repository name. Expected format: owner/repo".to_string(),
        ));
    }

    // Atomically claim the clone lock — compare_exchange ensures only one
    // concurrent request can start a clone (no TOCTOU gap).
    if state
        .clone_in_progress
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err((
            StatusCode::CONFLICT,
            "A clone operation is already in progress".to_string(),
        ));
    }

    let repo_path = {
        let config = state.config.read().await;
        std::path::PathBuf::from(&config.git.repo_path)
    };

    // Check if repo already exists — release the lock if we bail out early.
    if repo_path.join(".git").exists() && !body.force {
        state.clone_in_progress.store(false, Ordering::Release);
        return Err((
            StatusCode::CONFLICT,
            serde_json::json!({
                "error": "repository_exists",
                "message": format!("A repository already exists at {}", repo_path.display())
            })
            .to_string(),
        ));
    }

    let full_name = body.full_name.clone();
    let force = body.force;
    let state_clone = state.clone();

    // Spawn async clone task — use a drop guard so that clone_in_progress is
    // always cleared even if the task panics.
    tokio::spawn(async move {
        let _guard = CloneGuard(state_clone.clone_in_progress.clone());
        info!(repo = %full_name, force, "Starting async repository clone");
        let result = do_clone(&state_clone, &full_name, &repo_path, force).await;

        // Broadcast result via WebSocket and log outcome
        let event = match &result {
            Ok(()) => {
                info!(repo = %full_name, "Repository cloned successfully");
                maestro_core::workflow::engine::WorkflowEvent {
                    event_type: "repo_clone_progress".to_string(),
                    workflow_id: String::new(),
                    ticket_key: "__system__".to_string(),
                    state: "success".to_string(),
                    timestamp: chrono::Utc::now(),
                    error: None,
                    step_name: None,
                    output_line: Some(format!("Repository {} cloned successfully", full_name)),
                    stream: None,
                    progress_percent: None,
                    progress_steps_total: None,
                    forwarded_port: None,
                    pr_merged: None,
                }
            }
            Err(e) => {
                error!(repo = %full_name, error = %e, "Repository clone failed");
                maestro_core::workflow::engine::WorkflowEvent {
                    event_type: "repo_clone_progress".to_string(),
                    workflow_id: String::new(),
                    ticket_key: "__system__".to_string(),
                    state: "error".to_string(),
                    timestamp: chrono::Utc::now(),
                    error: Some(e.to_string()),
                    step_name: None,
                    output_line: None,
                    stream: None,
                    progress_percent: None,
                    progress_steps_total: None,
                    forwarded_port: None,
                    pr_merged: None,
                }
            }
        };

        state_clone.engine.broadcast_event(event);
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(CloneRepoResponse {
            status: "cloning".to_string(),
        }),
    ))
}

async fn do_clone(
    state: &AppState,
    full_name: &str,
    repo_path: &std::path::Path,
    force: bool,
) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // If force, remove existing repo
    if force && repo_path.exists() {
        tokio::fs::remove_dir_all(repo_path).await?;
        tokio::fs::create_dir_all(repo_path).await?;
    }

    // Ensure directory exists
    if !repo_path.exists() {
        tokio::fs::create_dir_all(repo_path).await?;
    }

    let gh_token = state
        .engine
        .actions
        .get_gh_installation_token(repo_path)
        .await;

    let clone_url = format!("https://github.com/{full_name}.git");

    if let Some(token) = gh_token {
        // Use git clone with an inline credential helper that echoes the GitHub App
        // installation token. Plain `git clone` does not read `GH_TOKEN` from the
        // environment, so we must configure a helper that supplies the token.
        let credential_helper = format!(
            "!f() {{ echo protocol=https; echo host=github.com; echo username=x-access-token; echo password={}; }}; f",
            token
        );
        let target = repo_path.to_str().unwrap_or(".");
        let output = maestro_core::process::run_command(
            "git",
            &[
                "-c",
                &format!("credential.helper={credential_helper}"),
                "clone",
                &clone_url,
                target,
            ],
            repo_path.parent().unwrap_or(std::path::Path::new("/")),
            tokio_util::sync::CancellationToken::new(),
        )
        .await?;
        if !output.success() {
            return Err(format!("git clone failed: {}", output.stderr.trim()).into());
        }
    } else {
        // Use gh repo clone
        let target = repo_path.to_str().unwrap_or(".");
        let output = maestro_core::process::run_command(
            "gh",
            &["repo", "clone", full_name, target],
            repo_path.parent().unwrap_or(std::path::Path::new("/")),
            tokio_util::sync::CancellationToken::new(),
        )
        .await?;
        if !output.success() {
            return Err(format!("gh repo clone failed: {}", output.stderr.trim()).into());
        }
    }

    // Set safe.directory
    let safe_dir = repo_path.to_str().unwrap_or(".");
    let _ = maestro_core::process::run_command(
        "git",
        &["config", "--global", "--add", "safe.directory", safe_dir],
        repo_path,
        tokio_util::sync::CancellationToken::new(),
    )
    .await;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::State;

    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use tokio::sync::RwLock;

    use maestro_core::actions::dry_run::DryRunActions;
    use maestro_core::config::{Config, TicketingSystem};
    use maestro_core::workflow::engine::WorkflowEngine;

    fn test_state() -> AppState {
        let config = Arc::new(RwLock::new(Config::default()));
        let actions: Arc<dyn maestro_core::actions::traits::ExternalActions> = Arc::new(
            DryRunActions::new(std::env::temp_dir(), "origin".to_string(), None),
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
        AppState {
            engine,
            config,
            polling_paused: Arc::new(AtomicBool::new(false)),
            jira_available,
            ticketing_system: TicketingSystem::None,
            editor_scanners: Arc::new(RwLock::new(HashMap::new())),
            dynamic_forwards: Arc::new(RwLock::new(HashMap::new())),
            terminal_ports: Arc::new(RwLock::new(HashMap::new())),
            run_commands: Arc::new(RwLock::new(HashMap::new())),
            preflight_error: None,
            config_path: std::env::temp_dir().join("config.toml"),
            config_writer: None,
            clone_in_progress: Arc::new(AtomicBool::new(false)),
        }
    }

    #[tokio::test]
    async fn clone_repo_rejects_empty_name() {
        let state = test_state();
        let result = clone_repo(
            State(state),
            Json(CloneRepoBody {
                full_name: "".to_string(),
                force: false,
            }),
        )
        .await;
        assert!(result.is_err());
        let (status, msg) = result.unwrap_err();
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(msg.contains("Invalid repository name"));
    }

    #[tokio::test]
    async fn clone_repo_rejects_name_without_slash() {
        let state = test_state();
        let result = clone_repo(
            State(state),
            Json(CloneRepoBody {
                full_name: "noslash".to_string(),
                force: false,
            }),
        )
        .await;
        assert!(result.is_err());
        let (status, _) = result.unwrap_err();
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn clone_repo_conflicts_when_already_in_progress() {
        let state = test_state();
        state.clone_in_progress.store(true, Ordering::Relaxed);
        let result = clone_repo(
            State(state),
            Json(CloneRepoBody {
                full_name: "owner/repo".to_string(),
                force: false,
            }),
        )
        .await;
        assert!(result.is_err());
        let (status, msg) = result.unwrap_err();
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(msg.contains("already in progress"));
    }
}
