// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::path::PathBuf;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Serialize;

use maestro_core::jira::client::{JiraClient, TicketDescriptionPreview};

use crate::state::AppState;

#[derive(Serialize)]
pub struct TodoTicketRow {
    pub key: String,
    pub summary: String,
    pub item_type: String,
}

/// All **To Do** issues for configured projects (every issue type), backlog order — for the manual-start picker.
pub async fn list_todo_tickets_manual(
    State(state): State<AppState>,
) -> Result<Json<Vec<TodoTicketRow>>, (StatusCode, String)> {
    let config = state.config.read().await;
    if config.jira.project_keys.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "No Jira project keys configured".to_string(),
        ));
    }
    let repo_path = PathBuf::from(&config.git.repo_path);
    let project_keys = config.jira.project_keys.clone();
    let jql_filter = config.jira.jql_filter.clone();
    drop(config);

    let client = JiraClient::new(repo_path);
    let tickets = client
        .list_todo_tickets_by_rank(&project_keys, &jql_filter)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;

    let rows: Vec<TodoTicketRow> = tickets
        .into_iter()
        .map(|t| TodoTicketRow {
            key: t.key,
            summary: t.summary,
            item_type: t.item_type,
        })
        .collect();

    Ok(Json(rows))
}

/// **Summary** and **description** for the manual-start detail modal (`description_markdown`: plain string or ADF → Markdown).
pub async fn get_ticket_preview(
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> Result<Json<TicketDescriptionPreview>, (StatusCode, String)> {
    let config = state.config.read().await;
    if config.jira.project_keys.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "No Jira project keys configured".to_string(),
        ));
    }
    let repo_path = PathBuf::from(&config.git.repo_path);
    let project_keys = config.jira.project_keys.clone();
    drop(config);

    let client = JiraClient::new(repo_path);
    let preview = client
        .get_ticket_description_preview(&key, &project_keys)
        .await
        .map_err(|e| {
            let msg = e.to_string();
            let code = if msg.contains("not in configured") {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::BAD_GATEWAY
            };
            (code, msg)
        })?;

    Ok(Json(preview))
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

    fn test_state_no_project_keys() -> AppState {
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
            clone_in_progress: Arc::new(AtomicBool::new(false)),
            path_token_registry: crate::session_registry::PathTokenRegistry::new(),
        }
    }

    fn test_state_with_project_keys(keys: Vec<String>) -> AppState {
        let mut cfg = Config::default();
        cfg.jira.project_keys = keys;
        let config = Arc::new(RwLock::new(cfg));
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
            clone_in_progress: Arc::new(AtomicBool::new(false)),
            path_token_registry: crate::session_registry::PathTokenRegistry::new(),
        }
    }

    #[tokio::test]
    async fn list_todo_tickets_returns_400_when_no_project_keys() {
        let state = test_state_no_project_keys();
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::get("/api/jira/todo-tickets-manual")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8_lossy(&body);
        assert!(
            text.contains("No Jira project keys"),
            "expected project keys error, got: {text}"
        );
    }

    #[tokio::test]
    async fn get_ticket_preview_returns_403_when_project_not_in_keys() {
        // Configure project keys as ["PROJ"] but request a ticket from "OTHER" project.
        let state = test_state_with_project_keys(vec!["PROJ".to_string()]);
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::get("/api/jira/tickets/OTHER-123/preview")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8_lossy(&body);
        assert!(
            text.contains("not in configured"),
            "expected 'not in configured' error, got: {text}"
        );
    }
}
