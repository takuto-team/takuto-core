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
}

/// `GET /api/github/issues` — returns open GitHub issues for the configured repo.
/// Returns `[{ "key": "GH-1", "summary": "...", "body": "..." }]`.
pub async fn list_github_issues(
    State(state): State<AppState>,
) -> Result<Json<Vec<GithubIssueRow>>, (StatusCode, String)> {
    let (repo_url, repo_path) = {
        let config = state.config.read().await;
        (
            config.git.repo_url.clone(),
            std::path::PathBuf::from(&config.git.repo_path),
        )
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

    let gh_token = state
        .engine
        .actions
        .get_gh_installation_token(&repo_path)
        .await;

    let issues = maestro_core::github::fetch_open_issues(&owner_repo, &repo_path, gh_token.as_deref())
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tokio::sync::RwLock;
    use tower::ServiceExt;

    use maestro_core::actions::dry_run::DryRunActions;
    use maestro_core::config::{Config, TicketingSystem};
    use maestro_core::workflow::engine::WorkflowEngine;

    use crate::server::build_router;
    use crate::state::AppState;

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
        }
    }

    #[tokio::test]
    async fn list_github_issues_returns_error_when_repo_url_empty() {
        // Default config has an empty repo_url, so parse_github_repo returns None → 400.
        let state = test_state();
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::get("/api/github/issues")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8_lossy(&body);
        assert!(
            text.contains("Cannot parse GitHub owner/repo"),
            "expected parse error, got: {text}"
        );
    }
}
