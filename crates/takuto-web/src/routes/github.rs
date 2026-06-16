// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use axum::Json;
use axum::extract::{Extension, Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::auth::AuthenticatedUser;
use crate::state::{AuthState, EngineState};

// Re-export so tickets.rs can import via `crate::routes::github::parse_github_repo`.
pub use takuto_core::github::parse_github_repo;

#[derive(Serialize, TS)]
#[ts(rename = "GitHubIssue", export_to = "GitHubIssue.ts")]
pub struct GithubIssueRow {
    pub key: String,
    pub summary: String,
    pub body: String,
    pub url: String,
}

#[cfg(test)]
mod ts_bindings {
    use super::*;
    use ts_rs::TS;

    /// Regenerate `ui/src/api/generated/GitHubIssue.ts` (CI diffs the dir).
    #[test]
    fn export_github_issue() {
        let out = crate::ts_bindings::generated_dir();
        std::fs::create_dir_all(&out).expect("create generated dir");
        GithubIssueRow::export_all_to(&out).expect("export GitHubIssue");
    }
}

#[derive(Deserialize)]
pub struct ListIssuesQuery {
    /// Required. The `repositories.name` (or `workspace_name` — same value)
    /// the caller has on their dashboard. Issues are listed for that repo.
    /// Without it we'd have to pick "the global repo" (no longer a concept)
    /// or list issues across every repo the caller has, which is both slow
    /// and surprising.
    pub repository: Option<String>,
}

/// `GET /api/github/issues?repository=<name>` — returns open GitHub issues for
/// the repository the caller has selected on their dashboard.
///
/// The caller must pass the repository name explicitly and must have it
/// associated in `user_repositories` (403 otherwise).
pub async fn list_github_issues(
    State(engine): State<EngineState>,
    State(auth_state): State<AuthState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Query(query): Query<ListIssuesQuery>,
) -> Result<Json<Vec<GithubIssueRow>>, (StatusCode, String)> {
    let repository = match query.repository.as_deref().map(str::trim) {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                "missing `repository` query param — pick a repository in the header before opening the picker".to_string(),
            ));
        }
    };

    let Some(db) = auth_state.db.clone() else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "database unavailable".into(),
        ));
    };

    let adapter = db.adapter();
    let has = takuto_core::db::repositories::user_has(adapter, &auth.user_id, &repository)
        .await
        .unwrap_or(false);
    let row_opt = if !has {
        None
    } else {
        takuto_core::db::repositories::get_by_name(adapter, &repository)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    };

    let Some(row) = row_opt else {
        return Err((
            StatusCode::FORBIDDEN,
            format!("repository `{repository}` is not on your dashboard"),
        ));
    };

    let repo_path = std::path::PathBuf::from(&row.local_path);
    let remote = "origin";
    let remote_url = takuto_core::git::remote::resolve_remote_url(&repo_path, remote)
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                format!("Cannot resolve git remote URL for `{}`: {e}", row.name),
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

    let app_token = engine
        .engine
        .actions()
        .get_gh_installation_token(&repo_path)
        .await;
    // PAT-only deployments have no App token — fall back to the caller's PAT.
    let gh_token = takuto_core::github::github_token_app_then_pat(
        app_token,
        auth_state.git_auth_resolver.as_ref(),
        Some(&auth.user_id),
        takuto_core::github::auth_resolver::GitAction::Clone,
    )
    .await;

    let issues =
        takuto_core::github::fetch_open_issues(&owner_repo, &repo_path, gh_token.as_deref())
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
    use crate::test_helpers::{TEST_ORIGIN, register_and_login, test_state_with_db};

    #[tokio::test]
    async fn list_github_issues_400_when_repository_query_missing() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::get("/api/github/issues")
                    .header("Cookie", &cookie)
                    .header("Origin", TEST_ORIGIN)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert!(String::from_utf8_lossy(&body).contains("missing `repository`"));
    }

    #[tokio::test]
    async fn list_github_issues_403_when_repository_not_associated() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::get("/api/github/issues?repository=not-mine")
                    .header("Cookie", &cookie)
                    .header("Origin", TEST_ORIGIN)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }
}
