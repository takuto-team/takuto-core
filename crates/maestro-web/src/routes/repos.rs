// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use std::sync::atomic::Ordering;
use tracing::{error, info};

// ── Workspace listing & switching ────────────────────────────────────────────

#[derive(Serialize)]
pub struct WorkspaceEntry {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html_url: Option<String>,
    pub active: bool,
}

/// `GET /api/workspaces` — list all git repos found under `/workspaces/`.
pub async fn list_workspaces(State(state): State<AppState>) -> Json<Vec<WorkspaceEntry>> {
    let active_path = state.config.read().await.git.repo_path.clone();
    let entries = std::fs::read_dir(WORKSPACES_DIR)
        .map(|dir| {
            let mut list: Vec<WorkspaceEntry> = dir
                .filter_map(|e| e.ok())
                .filter(|e| e.path().join(".git").exists())
                .map(|e| {
                    let path = e.path();
                    let name = e.file_name().to_string_lossy().into_owned();
                    let html_url = read_git_remote_url(&path);
                    let active = path.to_string_lossy() == active_path.as_str();
                    WorkspaceEntry {
                        name,
                        html_url,
                        active,
                    }
                })
                .collect();
            list.sort_by(|a, b| a.name.cmp(&b.name));
            list
        })
        .unwrap_or_default();
    Json(entries)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SwitchWorkspaceBody {
    pub name: String,
}

/// `POST /api/workspaces/switch` — make a local workspace the active repo.
pub async fn switch_workspace(
    State(state): State<AppState>,
    Json(body): Json<SwitchWorkspaceBody>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Reject any path-traversal attempt.
    if body.name.is_empty()
        || body.name.contains('/')
        || body.name.contains("..")
        || body.name.starts_with('.')
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "Invalid workspace name".to_string(),
        ));
    }

    let workspace_path = std::path::Path::new(WORKSPACES_DIR).join(&body.name);
    if !workspace_path.join(".git").exists() {
        return Err((
            StatusCode::NOT_FOUND,
            format!("No git repository found at {}", workspace_path.display()),
        ));
    }

    // Flush current workflows to per-workspace snapshots before switching.
    if let Err(e) = state.engine.sync_workflow_snapshot().await {
        tracing::warn!(error = %e, "Failed to sync workflow snapshot before workspace switch");
    }

    let new_path = workspace_path.to_string_lossy().into_owned();
    {
        let mut config = state.config.write().await;
        config.git.repo_path = new_path.clone();
    }
    // Persist the active workspace to the data dir (survives rebuilds).
    if let Err(e) = maestro_core::workflow::snapshot::write_active_workspace(&body.name) {
        tracing::warn!(error = %e, name = %body.name, "Failed to persist active workspace");
    }
    info!(name = %body.name, path = %new_path, "Active workspace switched");
    Ok(Json(serde_json::json!({ "repo_path": new_path })))
}

/// Read the `origin` remote URL from `.git/config` and normalise to an HTTPS GitHub URL.
pub(super) fn read_git_remote_url(repo_path: &std::path::Path) -> Option<String> {
    let git_config = std::fs::read_to_string(repo_path.join(".git/config")).ok()?;
    let mut in_origin = false;
    for line in git_config.lines() {
        let trimmed = line.trim();
        if trimmed == r#"[remote "origin"]"# {
            in_origin = true;
            continue;
        }
        if in_origin {
            if trimmed.starts_with('[') {
                break;
            }
            if let Some(rest) = trimmed.strip_prefix("url =") {
                return Some(normalize_github_url(rest.trim()));
            }
        }
    }
    None
}

fn normalize_github_url(url: &str) -> String {
    if let Some(path) = url.strip_prefix("git@github.com:") {
        return format!("https://github.com/{}", path.trim_end_matches(".git"));
    }
    if let Some(path) = url.strip_prefix("ssh://git@github.com/") {
        return format!("https://github.com/{}", path.trim_end_matches(".git"));
    }
    url.trim_end_matches(".git").to_string()
}

use crate::state::AppState;
use maestro_core::workflow::snapshot::WORKSPACES_DIR;

/// Strip lines from error messages that may contain credentials.
fn sanitize_clone_error(msg: &str) -> String {
    msg.lines()
        .filter(|line| {
            let lower = line.to_lowercase();
            !lower.contains("password") && !lower.contains("token") && !lower.contains("credential")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

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
    let workspaces = std::path::Path::new(WORKSPACES_DIR);

    let gh_token = state
        .engine
        .actions
        .get_gh_installation_token(workspaces)
        .await;

    let env: Vec<(&str, &str)> = gh_token
        .as_deref()
        .map(|t| vec![("GH_TOKEN", t)])
        .unwrap_or_default();

    // If a search query is provided, use the search endpoint.
    // For the empty-query case: GitHub App installation tokens cannot call
    // `user/repos` (returns "Resource not accessible by integration"), so use
    // `installation/repositories` instead. Fall back to `user/repos` only when
    // no installation token is available (i.e. plain `gh auth` login).
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
    } else if gh_token.is_some() {
        // Installation token: list repos the app installation has access to.
        vec![
            "api".to_string(),
            "installation/repositories".to_string(),
            "--method".to_string(),
            "GET".to_string(),
            "--field".to_string(),
            "per_page=100".to_string(),
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
        workspaces,
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

    // Handle all three response shapes:
    // - /user/repos              → JSON array
    // - /search/repositories     → { "items": [...] }
    // - /installation/repositories → { "repositories": [...] }
    let items = if let Some(arr) = json.as_array() {
        arr.clone()
    } else if let Some(items) = json.get("items").and_then(|v| v.as_array()) {
        items.clone()
    } else if let Some(repos) = json.get("repositories").and_then(|v| v.as_array()) {
        repos.clone()
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

/// `POST /api/repos/clone` — Clone a GitHub repo into `/workspaces/<repo-name>/`.
pub async fn clone_repo(
    State(state): State<AppState>,
    Json(body): Json<CloneRepoBody>,
) -> Result<(StatusCode, Json<CloneRepoResponse>), (StatusCode, String)> {
    // Validate full_name format: must be "owner/repo" with only safe characters.
    let parts: Vec<&str> = body.full_name.split('/').collect();
    let valid = parts.len() == 2
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
                && !p.starts_with('.')
        });
    if !valid {
        return Err((
            StatusCode::BAD_REQUEST,
            "Invalid repository name. Expected format: owner/repo".to_string(),
        ));
    }

    // Derive the clone destination: /workspaces/<repo-name>/
    let repo_name = parts[1];
    let clone_target = std::path::PathBuf::from(WORKSPACES_DIR).join(repo_name);

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

    // Check if a git repo already exists at the clone target — release the lock
    // if we bail out early.
    if clone_target.join(".git").exists() && !body.force {
        state.clone_in_progress.store(false, Ordering::Release);
        return Err((
            StatusCode::CONFLICT,
            serde_json::json!({
                "error": "repository_exists",
                "message": format!("A repository already exists at {}", clone_target.display())
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
        info!(repo = %full_name, target = %clone_target.display(), force, "Starting async repository clone");
        let result = do_clone(
            &state_clone,
            &full_name,
            std::path::Path::new(WORKSPACES_DIR),
            &clone_target,
            force,
        )
        .await;

        // On success, update config.git.repo_path to the new clone location and
        // persist the active workspace to the data dir.
        if result.is_ok() {
            let new_path = clone_target.to_string_lossy().into_owned();
            {
                let mut config = state_clone.config.write().await;
                config.git.repo_path = new_path.clone();
            }
            if let Some(name) = clone_target.file_name() {
                let _ = maestro_core::workflow::snapshot::write_active_workspace(
                    &name.to_string_lossy(),
                );
            }
        }

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
                    error: Some(sanitize_clone_error(&e.to_string())),
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

/// Perform the actual git clone.
///
/// `token_cwd` is an existing directory used as the working directory when
/// fetching the GitHub App installation token (pre-clone).
/// `clone_target` is the destination directory: `/workspaces/<repo-name>/`.
async fn do_clone(
    state: &AppState,
    full_name: &str,
    token_cwd: &std::path::Path,
    clone_target: &std::path::Path,
    force: bool,
) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // If force, wipe the existing directory before cloning.
    if force && clone_target.exists() {
        // Don't destroy the active workspace if workflows are running in it.
        let active_path = state.config.read().await.git.repo_path.clone();
        if clone_target.to_string_lossy() == active_path {
            let wf_arc = state.engine.workflows_arc();
            let workflows = wf_arc.read().await;
            let has_active = workflows.values().any(|w| {
                w.workspace_name
                    == maestro_core::workflow::snapshot::workspace_name_from_repo_path(
                        std::path::Path::new(&active_path),
                    )
                    && w.state.is_active()
            });
            if has_active {
                return Err(
                    "Cannot overwrite the active workspace while workflows are running. Stop all workflows first."
                        .into(),
                );
            }
        }
        tokio::fs::remove_dir_all(clone_target).await?;
    }

    // /workspaces is created at image build time and owned by the maestro user.
    // Use it as cwd for the installation token fetch.
    let gh_token = state
        .engine
        .actions
        .get_gh_installation_token(token_cwd)
        .await;

    let clone_url = format!("https://github.com/{full_name}.git");
    let target = clone_target.to_str().unwrap_or(".");
    let parent_dir = clone_target
        .parent()
        .unwrap_or(std::path::Path::new(WORKSPACES_DIR));

    use std::time::Duration;
    const CLONE_TIMEOUT: Duration = Duration::from_secs(600); // 10 minutes

    if let Some(token) = gh_token {
        // Use git clone with an inline credential helper that reads the token from
        // the GH_TOKEN environment variable (not embedded in the command line, so it
        // stays hidden from process listings).
        let credential_helper = "!f() { echo protocol=https; echo host=github.com; echo username=x-access-token; echo \"password=$GH_TOKEN\"; }; f";
        let output = tokio::time::timeout(
            CLONE_TIMEOUT,
            maestro_core::process::run_command_with_env(
                "git",
                &[
                    "-c",
                    &format!("credential.helper={credential_helper}"),
                    "clone",
                    &clone_url,
                    target,
                ],
                parent_dir,
                tokio_util::sync::CancellationToken::new(),
                &[("GH_TOKEN", &token)],
            ),
        )
        .await
        .map_err(|_| "git clone timed out after 10 minutes")??;
        if !output.success() {
            return Err(format!("git clone failed: {}", output.stderr.trim()).into());
        }
    } else {
        // Use gh repo clone
        let output = tokio::time::timeout(
            CLONE_TIMEOUT,
            maestro_core::process::run_command(
                "gh",
                &["repo", "clone", full_name, target],
                parent_dir,
                tokio_util::sync::CancellationToken::new(),
            ),
        )
        .await
        .map_err(|_| "gh repo clone timed out after 10 minutes")??;
        if !output.success() {
            return Err(format!("gh repo clone failed: {}", output.stderr.trim()).into());
        }
    }

    // Register the cloned directory as a git safe.directory.
    let _ = maestro_core::process::run_command(
        "git",
        &["config", "--global", "--add", "safe.directory", target],
        clone_target,
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
            db: None,
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
            path_token_registry: crate::session_registry::PathTokenRegistry::new(),
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

    #[test]
    fn normalize_ssh_git_at_url() {
        assert_eq!(
            normalize_github_url("git@github.com:owner/repo.git"),
            "https://github.com/owner/repo"
        );
    }

    #[test]
    fn normalize_ssh_protocol_url() {
        assert_eq!(
            normalize_github_url("ssh://git@github.com/owner/repo.git"),
            "https://github.com/owner/repo"
        );
    }

    #[test]
    fn normalize_https_url_with_git_suffix() {
        assert_eq!(
            normalize_github_url("https://github.com/owner/repo.git"),
            "https://github.com/owner/repo"
        );
    }

    #[test]
    fn normalize_https_url_without_git_suffix() {
        assert_eq!(
            normalize_github_url("https://github.com/owner/repo"),
            "https://github.com/owner/repo"
        );
    }

    #[test]
    fn normalize_non_github_url() {
        assert_eq!(
            normalize_github_url("https://gitlab.com/owner/repo.git"),
            "https://gitlab.com/owner/repo"
        );
    }
}
