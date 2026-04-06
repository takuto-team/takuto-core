use std::path::PathBuf;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::Serialize;

use maestro_core::jira::client::JiraClient;

use crate::state::AppState;

#[derive(Serialize)]
pub struct TodoTicketRow {
    pub key: String,
    pub summary: String,
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
    let acli_extras = config.jira.acli_extra_argv_prefixes();
    drop(config);

    let client = JiraClient::new(repo_path, acli_extras);
    let tickets = client
        .list_todo_tickets_by_rank(&project_keys, &jql_filter)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, e.to_string()))?;

    let rows: Vec<TodoTicketRow> = tickets
        .into_iter()
        .map(|t| TodoTicketRow {
            key: t.key,
            summary: t.summary,
        })
        .collect();

    Ok(Json(rows))
}
