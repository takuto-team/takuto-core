// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

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
    pub url: String,
}

/// `GET /api/github/issues` — returns open GitHub issues for the configured repo.
/// Returns `[{ "key": "GH-1", "summary": "...", "body": "..." }]`.
pub async fn list_github_issues(
    State(state): State<AppState>,
) -> Result<Json<Vec<GithubIssueRow>>, (StatusCode, String)> {
    let (repo_path, remote) = {
        let config = state.config.read().await;
        (
            std::path::PathBuf::from(&config.git.repo_path),
            config.git.remote.clone(),
        )
    };
    let remote_url = maestro_core::git::remote::resolve_remote_url(&repo_path, &remote)
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                format!("Cannot resolve git remote URL: {e}. Is a repository cloned?"),
            )
        })?;
    let owner_repo = parse_github_repo(&remote_url).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            format!(
                "Cannot parse GitHub owner/repo from git remote URL: {remote_url:?}. \
                 Expected a GitHub URL (https://github.com/owner/repo)"
            ),
        )
    })?;

    let gh_token = state
        .engine
        .actions
        .get_gh_installation_token(&repo_path)
        .await;

    let issues =
        maestro_core::github::fetch_open_issues(&owner_repo, &repo_path, gh_token.as_deref())
            .await
            .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;

    let rows: Vec<GithubIssueRow> = issues
        .into_iter()
        .map(|issue| GithubIssueRow {
            key: issue.key,
            summary: issue.summary,
            body: issue.body,
            url: issue.html_url,
        })
        .collect();

    Ok(Json(rows))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::server::build_router;
    use crate::test_helpers::{register_and_login, test_state_with_db};

    #[tokio::test]
    async fn list_github_issues_returns_error_when_no_git_repo() {
        // Config has repo_path pointing to a non-git directory, so resolve_remote_url fails -> 400.
        let state = test_state_with_db();
        {
            let mut cfg = state.config.write().await;
            cfg.git.repo_path = std::env::temp_dir()
                .join("maestro-test-no-repo")
                .to_string_lossy()
                .to_string();
        }
        let cookie = register_and_login(&state).await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::get("/api/github/issues")
                    .header("Cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8_lossy(&body);
        assert!(
            text.contains("Cannot resolve git remote URL"),
            "expected remote resolve error, got: {text}"
        );
    }
}
