use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::Serialize;

use crate::state::AppState;

// Re-export so tickets.rs can import via `crate::routes::github::parse_github_repo`.
pub use maestro_core::github::parse_github_repo;

#[derive(Serialize)]
pub struct GithubIssueRow {
    pub key: String,
    pub summary: String,
    pub body: String,
}

/// `GET /api/github/issues` — returns open GitHub issues for the configured repo.
/// Returns `[{ "key": "GH-1", "summary": "...", "body": "..." }]`.
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

    let issues = maestro_core::github::fetch_open_issues(&owner_repo)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;

    let rows: Vec<GithubIssueRow> = issues
        .into_iter()
        .map(|issue| GithubIssueRow {
            key: issue.key,
            summary: issue.summary,
            body: issue.body,
        })
        .collect();

    Ok(Json(rows))
}
