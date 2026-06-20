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
    /// The caller already has this issue on their board (non-`Done`); the picker
    /// disables the row with an "Already added" message.
    pub already_added: bool,
    /// The most recent PR a prior run recorded for this issue, if any; the
    /// picker prompts before re-adding (a new run opens a separate PR).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub existing_pr_url: Option<String>,
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

    let keys: Vec<String> = issues.iter().map(|i| i.key.clone()).collect();
    let wf_arc = engine.engine.workflows_arc();
    let annotations = crate::routes::workflows::annotate_candidates(
        &wf_arc,
        Some(&db),
        &auth.user_id,
        Some(&row.name),
        &keys,
    )
    .await;

    let rows: Vec<GithubIssueRow> = issues
        .into_iter()
        .map(|issue| {
            let ann = annotations.get(&issue.key).cloned().unwrap_or_default();
            GithubIssueRow {
                key: issue.key,
                summary: issue.summary,
                body: issue.body,
                url: issue.html_url,
                already_added: ann.already_added,
                existing_pr_url: ann.existing_pr_url,
            }
        })
        .collect();

    Ok(Json(rows))
}

#[derive(Deserialize)]
pub struct ExistingPrQuery {
    pub repository: Option<String>,
    pub ticket_key: Option<String>,
}

#[derive(Serialize)]
pub struct ExistingPrResponse {
    /// URL of a PR already open on GitHub for the ticket's canonical branch, or
    /// `null` when none is found (or the check could not run).
    pub pr_url: Option<String>,
}

/// `GET /api/github/existing-pr?repository=<name>&ticket_key=<key>` — best-effort
/// check, called by the add picker when a candidate row is clicked (alongside the
/// local DB/in-memory check), for a PR that already exists on GitHub for the
/// ticket's canonical `feat/<key>` / `fix/<key>` branch. Returns the PR URL or
/// `null`. Resolution / token / `gh` failures yield `null` (never an error to the
/// picker), so a failed check just means "no warning".
pub async fn existing_pr_for_ticket(
    State(engine): State<EngineState>,
    State(auth_state): State<AuthState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Query(query): Query<ExistingPrQuery>,
) -> Result<Json<ExistingPrResponse>, (StatusCode, String)> {
    let repository = query
        .repository
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or((
            StatusCode::BAD_REQUEST,
            "missing `repository` query param".to_string(),
        ))?;
    let ticket_key = query
        .ticket_key
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or((
            StatusCode::BAD_REQUEST,
            "missing `ticket_key` query param".to_string(),
        ))?;

    let Some(db) = auth_state.db.clone() else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "database unavailable".into(),
        ));
    };
    let adapter = db.adapter();
    let has = takuto_core::db::repositories::user_has(adapter, &auth.user_id, repository)
        .await
        .unwrap_or(false);
    let row_opt = if has {
        takuto_core::db::repositories::get_by_name(adapter, repository)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    } else {
        None
    };
    let Some(row) = row_opt else {
        return Err((
            StatusCode::FORBIDDEN,
            format!("repository `{repository}` is not on your dashboard"),
        ));
    };

    let pr_url = ticket_existing_pr_url(
        &engine,
        &auth_state,
        &auth.user_id,
        std::path::Path::new(&row.local_path),
        ticket_key,
    )
    .await;
    Ok(Json(ExistingPrResponse { pr_url }))
}

/// Best-effort: the URL of a PR already on GitHub for `ticket_key`'s canonical
/// `feat/`/`fix/` branch, or `None`. Any failure — no remote, unparseable
/// owner/repo, no token, or a `gh` error — yields `None` (advisory only).
async fn ticket_existing_pr_url(
    engine: &EngineState,
    auth_state: &AuthState,
    user_id: &str,
    repo_path: &std::path::Path,
    ticket_key: &str,
) -> Option<String> {
    let remote_url = takuto_core::git::remote::resolve_remote_url(repo_path, "origin")
        .await
        .ok()?;
    let owner_repo = takuto_core::github::parse_github_repo(&remote_url)?;
    let app_token = engine
        .engine
        .actions()
        .get_gh_installation_token(repo_path)
        .await;
    let gh_token = takuto_core::github::github_token_app_then_pat(
        app_token,
        auth_state.git_auth_resolver.as_ref(),
        Some(user_id),
        takuto_core::github::auth_resolver::GitAction::Clone,
    )
    .await;
    // Item type isn't known at pick-time; check both branch-prefix conventions.
    for item_type in ["Task", "Bug"] {
        let branch = takuto_core::git::worktree::branch_name_for_ticket(ticket_key, item_type);
        if let Ok(Some(url)) = takuto_core::github::find_pr_url_for_branch(
            &owner_repo,
            repo_path,
            &branch,
            gh_token.as_deref(),
        )
        .await
        {
            return Some(url);
        }
    }
    None
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

    #[tokio::test]
    async fn existing_pr_400_when_ticket_key_missing() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::get("/api/github/existing-pr?repository=repo")
                    .header("Cookie", &cookie)
                    .header("Origin", TEST_ORIGIN)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        assert!(String::from_utf8_lossy(&body).contains("ticket_key"));
    }

    #[tokio::test]
    async fn existing_pr_403_when_repository_not_associated() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::get("/api/github/existing-pr?repository=not-mine&ticket_key=GH-1")
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
