// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

pub mod poller;
pub mod pr_merge_poller;

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
}

/// Fetch open GitHub issues using `gh api`. Returns issues as key/summary/body.
///
/// Shared by both the GitHub poller (auto-starting workflows) and the
/// `GET /api/github/issues` route (dashboard issue picker).
pub async fn fetch_open_issues(
    owner_repo: &str,
    cwd: &std::path::Path,
) -> crate::error::Result<Vec<GitHubIssue>> {
    let endpoint = format!("repos/{owner_repo}/issues");
    let output = crate::process::run_command(
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
    )
    .await?;

    if !output.success() {
        return Err(crate::error::MaestroError::Config(format!(
            "gh api repos/{owner_repo}/issues failed: {}",
            output.stderr.trim()
        )));
    }

    let json: serde_json::Value = serde_json::from_str(output.stdout.trim())
        .map_err(|e| crate::error::MaestroError::Config(e.to_string()))?;

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
                    Some(GitHubIssue {
                        key: format!("GH-{number}"),
                        summary: title,
                        body,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Ok(issues)
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
}
