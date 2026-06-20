// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.
pub mod auth_resolver;
pub mod poller;
pub mod pr_merge_poller;

use std::sync::Arc;

use auth_resolver::{GitAction, GitAuthResolver};

/// Resolve a GitHub token for a server-side `gh` / REST call: prefer the
/// already-fetched GitHub **App** installation token; when there is none (a
/// PAT-only deployment), fall back to the caller's per-user PAT via the
/// [`GitAuthResolver`]. Returns `None` only when neither is available.
///
/// Centralises the App-then-PAT precedence so every GitHub-touching call site
/// (repo listing, issue picker, description sync, mergers/pollers) authenticates
/// identically and never runs `gh` unauthenticated on a PAT-only install.
pub async fn github_token_app_then_pat(
    app_token: Option<String>,
    resolver: Option<&Arc<GitAuthResolver>>,
    user_id: Option<&str>,
    action: GitAction,
) -> Option<String> {
    if app_token.is_some() {
        return app_token;
    }
    match (resolver, user_id) {
        (Some(resolver), Some(uid)) => resolver
            .token_for(action, uid)
            .await
            .ok()
            .map(|t| t.bearer.expose().to_string()),
        _ => None,
    }
}

/// Parse `owner/repo` from a GitHub URL or bare `owner/repo` string.
///
/// Handles:
/// - `https://github.com/owner/repo`
/// - `https://github.com/owner/repo.git`
/// - `git@github.com:owner/repo`
/// - `owner/repo` (bare)
pub fn parse_github_repo(repo_url: &str) -> Option<String> {
    let url = repo_url
        .trim()
        .trim_end_matches('/')
        .trim_end_matches(".git");
    if let Some(rest) = url.strip_prefix("https://github.com/")
        && rest.contains('/')
    {
        return Some(rest.to_string());
    }
    if let Some(rest) = url.strip_prefix("git@github.com:")
        && rest.contains('/')
    {
        return Some(rest.to_string());
    }
    // bare "owner/repo"
    if url.contains('/') && !url.contains("://") {
        return Some(url.to_string());
    }
    None
}

/// Parse a GitHub pull request URL into `(owner/repo, pr_number)`.
///
/// Handles: `https://github.com/{owner}/{repo}/pull/{number}` (with optional trailing segments).
/// Returns `None` for non-matching URLs.
pub fn parse_pr_url(url: &str) -> Option<(String, u64)> {
    let url = url.trim().trim_end_matches('/');
    let rest = url.strip_prefix("https://github.com/")?;
    // rest = "owner/repo/pull/123" or "owner/repo/pull/123/files"
    let mut parts = rest.splitn(4, '/');
    let owner = parts.next().filter(|s| !s.is_empty())?;
    let repo = parts.next().filter(|s| !s.is_empty())?;
    let pull_segment = parts.next().filter(|&s| s == "pull")?;
    let _ = pull_segment; // consumed to verify
    let number_segment = parts.next()?;
    // number_segment might be "123" or "123/files" — extract leading digits
    let number_str = number_segment.split('/').next()?;
    let number: u64 = number_str.parse().ok()?;
    Some((format!("{owner}/{repo}"), number))
}

/// A GitHub issue fetched from the GitHub API.
#[derive(Debug, Clone)]
pub struct GitHubIssue {
    pub key: String,
    pub summary: String,
    pub body: String,
    pub html_url: String,
    /// Label names attached to the issue (used by the polling label filter).
    pub labels: Vec<String>,
}

/// Translate a raw `gh api` error message into a user-friendly string.
///
/// GitHub returns "Resource not accessible by integration (HTTP 403)" when the
/// GitHub App installation is missing the required permission for the endpoint.
/// This turns that opaque message into an actionable instruction.
pub fn gh_api_error_message(raw_stderr: &str, required_permission: &str) -> String {
    if raw_stderr.contains("Resource not accessible by integration")
        || (raw_stderr.contains("403") && raw_stderr.contains("integration"))
    {
        format!(
            "GitHub App is missing the '{required_permission}' permission. \
             Go to your GitHub App settings → Permissions & events → set \
             '{required_permission}' to Read (or Write), save, then re-approve \
             the installation on your account/org."
        )
    } else if raw_stderr.contains("401") || raw_stderr.contains("Bad credentials") {
        "GitHub authentication failed. The GitHub App token may be invalid or expired. \
         Check that app_id, app_installation_id, and the private key are correct in config.toml."
            .to_string()
    } else {
        raw_stderr.to_string()
    }
}

/// Fetch open GitHub issues using `gh api`. Returns issues as key/summary/body.
///
/// Shared by both the GitHub poller (auto-starting workflows) and the
/// `GET /api/github/issues` route (dashboard issue picker).
///
/// When `gh_token` is `Some`, it is injected as `GH_TOKEN` so the call works
/// with a GitHub App installation token even if `gh auth` was never set up.
pub async fn fetch_open_issues(
    owner_repo: &str,
    cwd: &std::path::Path,
    gh_token: Option<&str>,
) -> crate::error::Result<Vec<GitHubIssue>> {
    let endpoint = format!("repos/{owner_repo}/issues");
    let env: Vec<(&str, &str)> = gh_token.map(|t| vec![("GH_TOKEN", t)]).unwrap_or_default();
    let output = crate::process::run_command_with_env(
        "gh",
        &[
            "api",
            "--method",
            "GET",
            &endpoint,
            "--field",
            "state=open",
            "--field",
            "per_page=50",
        ],
        cwd,
        tokio_util::sync::CancellationToken::new(),
        &env,
    )
    .await?;

    if !output.success() {
        return Err(crate::config::ConfigError::Operational {
            op: "gh api repos/<owner_repo>/issues",
            detail: gh_api_error_message(output.stderr.trim(), "Issues: Read"),
        }
        .into());
    }

    let json: serde_json::Value = serde_json::from_str(output.stdout.trim()).map_err(|e| {
        crate::config::ConfigError::Operational {
            op: "github issues json parse",
            detail: e.to_string(),
        }
    })?;

    let issues = json
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    // Skip pull requests (GitHub API returns PRs in issues endpoint)
                    if v.get("pull_request").is_some() {
                        return None;
                    }
                    let number = v.get("number")?.as_u64()?;
                    let title = v.get("title")?.as_str().unwrap_or("").to_string();
                    let body = v
                        .get("body")
                        .and_then(|b| b.as_str())
                        .unwrap_or("")
                        .to_string();
                    let html_url = v
                        .get("html_url")
                        .and_then(|u| u.as_str())
                        .unwrap_or("")
                        .to_string();
                    let labels = v
                        .get("labels")
                        .and_then(|l| l.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|lbl| {
                                    lbl.get("name").and_then(|n| n.as_str()).map(String::from)
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    Some(GitHubIssue {
                        key: format!("GH-{number}"),
                        summary: title,
                        body,
                        html_url,
                        labels,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Ok(issues)
}

/// Classify the outcome of a `gh api repos/{owner}/{repo}` probe.
///
/// `Some(true)` = accessible; **`Some(false)` only on a definitive
/// not-found/forbidden** (the App/PAT genuinely can't see the repo);
/// `None` = indeterminate (network/transient/`gh` error) so callers can avoid
/// false-flagging a repo as inaccessible on a hiccup.
pub fn classify_repo_access(success: bool, stderr: &str) -> Option<bool> {
    if success {
        return Some(true);
    }
    let s = stderr.to_lowercase();
    let denied = s.contains("404")
        || s.contains("not found")
        || s.contains("403")
        || s.contains("resource not accessible");
    if denied { Some(false) } else { None }
}

/// Probe whether the current token can see `owner_repo` via
/// `gh api repos/{owner_repo}`. See [`classify_repo_access`] for the tri-state
/// return. `gh_token` is injected as `GH_TOKEN` (App installation token or PAT).
pub async fn repo_accessible(
    owner_repo: &str,
    cwd: &std::path::Path,
    gh_token: Option<&str>,
) -> Option<bool> {
    let endpoint = format!("repos/{owner_repo}");
    let env: Vec<(&str, &str)> = gh_token.map(|t| vec![("GH_TOKEN", t)]).unwrap_or_default();
    let output = crate::process::run_command_with_env(
        "gh",
        &["api", "--method", "GET", &endpoint, "--jq", ".id"],
        cwd,
        tokio_util::sync::CancellationToken::new(),
        &env,
    )
    .await
    .ok()?;
    classify_repo_access(output.success(), output.stderr.trim())
}

/// The `head` filter value (`owner:branch`) for the GitHub pulls API, derived
/// from an `owner/repo` string. `None` when `owner_repo` has no owner segment.
pub fn pr_head_filter(owner_repo: &str, branch: &str) -> Option<String> {
    let (owner, repo) = owner_repo.split_once('/')?;
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some(format!("{owner}:{branch}"))
}

/// Find a PR (any state) whose head branch is `branch` in `owner_repo`, returning
/// its `html_url` if one exists. Used at add-time to warn when a ticket already
/// has a PR opened directly on GitHub (outside Takuto's DB / in-memory state).
///
/// `gh_token` is injected as `GH_TOKEN` (App installation token or per-user PAT)
/// so the call authenticates on deployments where `gh auth` was never set up.
pub async fn find_pr_url_for_branch(
    owner_repo: &str,
    cwd: &std::path::Path,
    branch: &str,
    gh_token: Option<&str>,
) -> crate::error::Result<Option<String>> {
    let Some(head) = pr_head_filter(owner_repo, branch) else {
        return Ok(None);
    };
    let endpoint = format!("repos/{owner_repo}/pulls");
    let head_field = format!("head={head}");
    let env: Vec<(&str, &str)> = gh_token.map(|t| vec![("GH_TOKEN", t)]).unwrap_or_default();
    let output = crate::process::run_command_with_env(
        "gh",
        &[
            "api",
            "--method",
            "GET",
            &endpoint,
            "--field",
            &head_field,
            "--field",
            "state=all",
            "--field",
            "per_page=1",
        ],
        cwd,
        tokio_util::sync::CancellationToken::new(),
        &env,
    )
    .await?;

    if !output.success() {
        return Err(crate::config::ConfigError::Operational {
            op: "gh api repos/<owner_repo>/pulls",
            detail: gh_api_error_message(output.stderr.trim(), "Pull requests: Read"),
        }
        .into());
    }

    let json: serde_json::Value = serde_json::from_str(output.stdout.trim()).map_err(|e| {
        crate::config::ConfigError::Operational {
            op: "github pulls json parse",
            detail: e.to_string(),
        }
    })?;

    let url = json
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|pr| pr.get("html_url"))
        .and_then(|u| u.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from);
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_pr_url_standard() {
        assert_eq!(
            parse_pr_url("https://github.com/owner/repo/pull/123"),
            Some(("owner/repo".to_string(), 123))
        );
    }

    #[test]
    fn parse_pr_url_trailing_slash() {
        assert_eq!(
            parse_pr_url("https://github.com/owner/repo/pull/123/"),
            Some(("owner/repo".to_string(), 123))
        );
    }

    #[test]
    fn parse_pr_url_with_extra_segments() {
        assert_eq!(
            parse_pr_url("https://github.com/owner/repo/pull/123/files"),
            Some(("owner/repo".to_string(), 123))
        );
    }

    #[test]
    fn parse_pr_url_hyphenated_names() {
        assert_eq!(
            parse_pr_url("https://github.com/my-org/my-repo/pull/42"),
            Some(("my-org/my-repo".to_string(), 42))
        );
    }

    #[test]
    fn parse_pr_url_issue_url_returns_none() {
        assert_eq!(
            parse_pr_url("https://github.com/owner/repo/issues/123"),
            None
        );
    }

    #[test]
    fn parse_pr_url_non_github_returns_none() {
        assert_eq!(
            parse_pr_url("https://gitlab.com/owner/repo/merge_requests/1"),
            None
        );
    }

    #[test]
    fn parse_pr_url_empty_returns_none() {
        assert_eq!(parse_pr_url(""), None);
    }

    #[test]
    fn parse_pr_url_bare_owner_repo_returns_none() {
        assert_eq!(parse_pr_url("owner/repo"), None);
    }

    #[test]
    fn parse_pr_url_non_numeric_returns_none() {
        assert_eq!(parse_pr_url("https://github.com/owner/repo/pull/abc"), None);
    }

    #[test]
    fn pr_head_filter_builds_owner_colon_branch() {
        assert_eq!(
            pr_head_filter("octocat/hello", "feat/gh-15"),
            Some("octocat:feat/gh-15".to_string())
        );
    }

    #[test]
    fn pr_head_filter_none_without_owner() {
        assert_eq!(pr_head_filter("noslash", "feat/x"), None);
        assert_eq!(pr_head_filter("/repo", "feat/x"), None);
    }

    #[test]
    fn classify_repo_access_success_is_accessible() {
        assert_eq!(classify_repo_access(true, ""), Some(true));
    }

    #[test]
    fn classify_repo_access_404_403_is_denied() {
        assert_eq!(
            classify_repo_access(false, "gh: Not Found (HTTP 404)"),
            Some(false)
        );
        assert_eq!(
            classify_repo_access(false, "HTTP 403: forbidden"),
            Some(false)
        );
        assert_eq!(
            classify_repo_access(false, "Resource not accessible by integration"),
            Some(false)
        );
    }

    #[test]
    fn classify_repo_access_transient_is_indeterminate() {
        assert_eq!(
            classify_repo_access(false, "could not resolve host: api.github.com"),
            None
        );
        assert_eq!(classify_repo_access(false, ""), None);
    }

    // ── github_token_app_then_pat: App-token-first, per-user PAT fallback ──────
    // Regression guard for "use the per-user PAT for every server-side GitHub
    // call" — every GitHub-touching call site (repo list, issue picker,
    // description sync, pollers) routes through this helper.

    async fn resolver_with_pat(user_id: &str) -> Arc<GitAuthResolver> {
        use crate::auth::{MasterKey, seal};
        use crate::db::{Database, DbValue, github_credentials};
        let db = Database::open_in_memory()
            .expect("in-mem db")
            .with_test_master_key(MasterKey::from_bytes([0x42; 32]));
        db.adapter()
            .execute(
                "INSERT INTO users (id, username, role) VALUES (?, ?, 'user')",
                vec![
                    DbValue::Text(user_id.to_string()),
                    DbValue::Text(user_id.to_string()),
                ],
            )
            .await
            .unwrap();
        let mk = db.master_key().expect("mk").key.clone();
        let sealed = seal(&mk, b"ghp_user_pat").unwrap();
        let mut tx = db.adapter().begin().await.unwrap();
        github_credentials::upsert(
            &mut tx,
            user_id,
            &sealed,
            &format!("{user_id}-gh"),
            "[\"repo\"]",
            true,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        Arc::new(GitAuthResolver::new(db, None))
    }

    #[tokio::test]
    async fn token_app_then_pat_prefers_app_token() {
        // App token present → returned as-is; the resolver is never consulted
        // (passing None proves the App path short-circuits).
        let got =
            github_token_app_then_pat(Some("app-tok".into()), None, None, GitAction::Clone).await;
        assert_eq!(got.as_deref(), Some("app-tok"));
    }

    #[tokio::test]
    async fn token_app_then_pat_falls_back_to_user_pat() {
        // No App token (PAT-only deployment) → resolve the caller's PAT.
        let resolver = resolver_with_pat("u-1").await;
        let got =
            github_token_app_then_pat(None, Some(&resolver), Some("u-1"), GitAction::Clone).await;
        assert_eq!(got.as_deref(), Some("ghp_user_pat"));
    }

    #[tokio::test]
    async fn token_app_then_pat_none_without_app_or_resolver() {
        assert!(
            github_token_app_then_pat(None, None, Some("u-1"), GitAction::Clone)
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn token_app_then_pat_none_without_user_id() {
        let resolver = resolver_with_pat("u-1").await;
        assert!(
            github_token_app_then_pat(None, Some(&resolver), None, GitAction::Clone)
                .await
                .is_none()
        );
    }
}
