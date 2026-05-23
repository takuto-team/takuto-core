// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Plan-10: workspace listing/switching and `POST /api/repos/clone` are hard
//! deleted (decision #3). The previously admin-gated route handlers have moved
//! out of this module entirely; what remains are the `pub(crate)` helpers
//! reused by `routes/repositories.rs::POST /api/repositories` (clone-if-needed
//! flow) and the still-mounted `GET /api/github/repos` listing endpoint.

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use std::sync::atomic::Ordering;

use crate::state::AppState;
use maestro_core::workflow::snapshot::WORKSPACES_DIR;

// ── GitHub repo listing (unchanged from plan-09) ─────────────────────────────

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
        .actions()
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

// ── pub(crate) helpers reused by `routes/repositories.rs` ────────────────────

/// Read the `origin` remote URL from `.git/config` and normalise to an HTTPS GitHub URL.
pub(crate) fn read_git_remote_url(repo_path: &std::path::Path) -> Option<String> {
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

pub(crate) fn normalize_github_url(url: &str) -> String {
    if let Some(path) = url.strip_prefix("git@github.com:") {
        return format!("https://github.com/{}", path.trim_end_matches(".git"));
    }
    if let Some(path) = url.strip_prefix("ssh://git@github.com/") {
        return format!("https://github.com/{}", path.trim_end_matches(".git"));
    }
    url.trim_end_matches(".git").to_string()
}

/// Strip lines from error messages that may contain credentials.
pub(crate) fn sanitize_clone_error(msg: &str) -> String {
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
pub(crate) struct CloneGuard(pub std::sync::Arc<std::sync::atomic::AtomicBool>);
impl Drop for CloneGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// Perform a git clone of `full_name` (`owner/repo`) into `clone_target`.
///
/// `token_cwd` is an existing directory used as the working directory when
/// fetching the GitHub App installation token (pre-clone). `user_id` is the
/// authenticated caller (`Some` for user-driven clones via
/// `POST /api/repositories`; `None` for poller / legacy paths).
///
/// Phase 2b.2: when `user_id.is_some()` AND the `GitAuthResolver` is wired,
/// we ask the resolver for a `GitAction::Clone` token (which picks App in
/// Mode A/B and UserPat in Mode C per arch §4.2). Otherwise we fall back
/// to the legacy `actions.get_gh_installation_token` + `gh repo clone`
/// chain that pre-existed Phase 2b.2.
///
/// This is the helper used by `routes/repositories.rs::POST /api/repositories`
/// to perform the actual filesystem clone after the per-process lock has been
/// acquired and the destination path resolved.
pub(crate) async fn do_clone(
    state: &AppState,
    full_name: &str,
    token_cwd: &std::path::Path,
    clone_target: &std::path::Path,
    user_id: Option<&str>,
) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use maestro_core::github::auth_resolver::GitAction;

    // 1. Prefer the resolver path when both a user_id and a resolver exist.
    let gh_token = match (user_id, state.git_auth_resolver.as_ref()) {
        (Some(uid), Some(resolver)) => match resolver.token_for(GitAction::Clone, uid).await {
            Ok(tok) => Some(tok.bearer.expose().to_string()),
            // Resolver returned UnauthenticatedGit (Mode Missing) — surface
            // as a clean clone failure rather than crashing.
            Err(maestro_core::github::auth_resolver::GitAuthError::UnauthenticatedGit { .. }) => {
                return Err(
                    "GitHub authentication unavailable for this user — paste a PAT in My Credentials or configure a GitHub App.".into(),
                );
            }
            Err(e) => return Err(format!("token resolution failed: {e}").into()),
        },
        // 2. Legacy path: ask the App directly via the existing actions trait.
        //    Mode `db: None` tests and pre-Phase-2b.2 poller calls go through here.
        _ => state
            .engine
            .actions()
            .get_gh_installation_token(token_cwd)
            .await,
    };

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

    #[test]
    fn sanitize_clone_error_strips_credential_lines() {
        let msg = "fatal: clone failed\nremote: password not accepted\nrun git -c credential.helper\nremote: token expired";
        let sanitized = sanitize_clone_error(msg);
        assert!(sanitized.contains("clone failed"));
        assert!(!sanitized.contains("password"));
        assert!(!sanitized.contains("token"));
        assert!(!sanitized.contains("credential"));
    }
}
