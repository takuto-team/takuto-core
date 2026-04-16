use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::Serialize;

use crate::state::AppState;

#[derive(Serialize)]
pub struct GithubIssueRow {
    pub key: String,
    pub summary: String,
}

/// Parse `owner/repo` from a GitHub URL or bare `owner/repo` string.
fn parse_github_repo(repo_url: &str) -> Option<String> {
    let url = repo_url.trim().trim_end_matches('/').trim_end_matches(".git");
    if let Some(rest) = url.strip_prefix("https://github.com/") {
        if rest.contains('/') {
            return Some(rest.to_string());
        }
    }
    if let Some(rest) = url.strip_prefix("git@github.com:") {
        if rest.contains('/') {
            return Some(rest.to_string());
        }
    }
    if url.contains('/') && !url.contains("://") {
        return Some(url.to_string());
    }
    None
}

/// `GET /api/github/issues` — returns open GitHub issues for the configured repo.
/// Returns `[{ "key": "GH-1", "summary": "..." }]`.
pub async fn list_github_issues(
    State(state): State<AppState>,
) -> Result<Json<Vec<GithubIssueRow>>, (StatusCode, String)> {
    let repo_url = {
        let config = state.config.read().await;
        config.git.repo_url.clone()
    };

    let owner_repo = parse_github_repo(&repo_url).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            format!(
                "Cannot parse GitHub owner/repo from git.repo_url: {repo_url:?}. \
                 Expected format: https://github.com/owner/repo or owner/repo"
            ),
        )
    })?;

    let output = tokio::process::Command::new("gh")
        .args([
            "api",
            &format!("repos/{owner_repo}/issues"),
            "--field",
            "state=open",
            "--field",
            "per_page=50",
            "--field",
            "filter=all",
        ])
        .output()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err((
            StatusCode::BAD_GATEWAY,
            format!("gh api repos/{owner_repo}/issues failed: {stderr}"),
        ));
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;

    let rows: Vec<GithubIssueRow> = json
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    // Skip pull requests (GitHub API returns PRs in the issues endpoint)
                    if v.get("pull_request").is_some() {
                        return None;
                    }
                    let number = v.get("number")?.as_u64()?;
                    let title = v.get("title")?.as_str().unwrap_or("").to_string();
                    Some(GithubIssueRow {
                        key: format!("GH-{number}"),
                        summary: title,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(Json(rows))
}
