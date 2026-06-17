// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::path::PathBuf;

use axum::Json;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use serde::Serialize;
use ts_rs::TS;

use takuto_core::jira::client::{JiraClient, TicketDescriptionPreview};

use crate::auth::AuthenticatedUser;
use crate::state::{AuthState, ConfigState, EngineState};

#[derive(Serialize, TS)]
#[ts(rename = "TodoTicket", export_to = "TodoTicket.ts")]
pub struct TodoTicketRow {
    pub key: String,
    pub summary: String,
    pub item_type: String,
    /// The caller already has this ticket on their board (non-`Done`); the
    /// picker disables the row with an "Already added" message.
    pub already_added: bool,
    /// The most recent PR a prior run recorded for this ticket, if any; the
    /// picker prompts before re-adding (a new run opens a separate PR).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub existing_pr_url: Option<String>,
}

#[cfg(test)]
mod ts_bindings {
    use super::*;
    use ts_rs::TS;

    /// Regenerate `ui/src/api/generated/TodoTicket.ts` (CI diffs the dir).
    #[test]
    fn export_todo_ticket() {
        let out = crate::ts_bindings::generated_dir();
        std::fs::create_dir_all(&out).expect("create generated dir");
        TodoTicketRow::export_all_to(&out).expect("export TodoTicket");
    }
}

/// All **To Do** issues for configured projects (every issue type), backlog order — for the manual-start picker.
pub async fn list_todo_tickets_manual(
    State(cfg): State<ConfigState>,
    State(engine): State<EngineState>,
    State(auth_state): State<AuthState>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<Json<Vec<TodoTicketRow>>, (StatusCode, String)> {
    let config = cfg.config.read().await;
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

    // Jira keys are globally unique within the instance, so annotation is not
    // workspace-scoped (no repo context in this endpoint).
    let keys: Vec<String> = tickets.iter().map(|t| t.key.clone()).collect();
    let wf_arc = engine.engine.workflows_arc();
    let annotations = crate::routes::workflows::annotate_candidates(
        &wf_arc,
        auth_state.db.as_ref(),
        &auth.user_id,
        None,
        &keys,
    )
    .await;

    let rows: Vec<TodoTicketRow> = tickets
        .into_iter()
        .map(|t| {
            let ann = annotations.get(&t.key).cloned().unwrap_or_default();
            TodoTicketRow {
                key: t.key,
                summary: t.summary,
                item_type: t.item_type,
                already_added: ann.already_added,
                existing_pr_url: ann.existing_pr_url,
            }
        })
        .collect();

    Ok(Json(rows))
}

/// **Summary** and **description** for the manual-start detail modal (`description_markdown`: plain string or ADF → Markdown).
pub async fn get_ticket_preview(
    State(cfg): State<ConfigState>,
    Path(key): Path<String>,
) -> Result<Json<TicketDescriptionPreview>, (StatusCode, String)> {
    let config = cfg.config.read().await;
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
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::server::build_router;
    use crate::test_helpers::{register_and_login, test_state_with_db};

    #[tokio::test]
    async fn list_todo_tickets_returns_400_when_no_project_keys() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::get("/api/jira/todo-tickets-manual")
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
            text.contains("No Jira project keys"),
            "expected project keys error, got: {text}"
        );
    }

    #[tokio::test]
    async fn get_ticket_preview_returns_403_when_project_not_in_keys() {
        // Configure project keys as ["PROJ"] but request a ticket from "OTHER" project.
        let state = test_state_with_db();
        {
            let mut cfg = state.config.config.write().await;
            cfg.jira.project_keys = vec!["PROJ".to_string()];
        }
        let cookie = register_and_login(&state).await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::get("/api/jira/tickets/OTHER-123/preview")
                    .header("Cookie", &cookie)
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
