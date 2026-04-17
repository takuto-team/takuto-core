pub mod poller;

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
pub async fn fetch_open_issues(owner_repo: &str) -> crate::error::Result<Vec<GitHubIssue>> {
    let output = tokio::process::Command::new("gh")
        .args([
            "api",
            "--method",
            "GET",
            &format!("repos/{owner_repo}/issues"),
            "--field",
            "state=open",
            "--field",
            "per_page=50",
        ])
        .output()
        .await
        .map_err(|e| crate::error::MaestroError::Config(e.to_string()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(crate::error::MaestroError::Config(format!(
            "gh api repos/{owner_repo}/issues failed: {stderr}"
        )));
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
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
