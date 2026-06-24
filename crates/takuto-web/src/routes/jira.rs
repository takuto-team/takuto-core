// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::path::PathBuf;

use axum::Json;
use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use takuto_core::db::user_repo_polling_settings;
use takuto_core::jira::client::{JiraClient, TicketDescriptionPreview};
use takuto_core::jira::{JiraRestClient, resolve_rest_credential};

use crate::auth::AuthenticatedUser;
use crate::state::{AuthState, ConfigState, EngineState};

/// Error type for Jira route handlers that perform a per-user REST call.
///
/// Auth failures on the REST path (Jira 401/403, only reachable when a token is
/// set) become a structured **`401 { "code": "jira_credential_invalid", … }`**
/// body that drives the global "Jira authentication failed" modal. Every other
/// failure stays a plain-text body at its existing status (unchanged behaviour).
pub enum JiraRouteError {
    /// Jira rejected the stored token (upstream 401/403) → modal code.
    CredentialInvalid { status: u16 },
    /// Any other error — preserved as plain text at the given status.
    Plain(StatusCode, String),
}

impl From<(StatusCode, String)> for JiraRouteError {
    fn from((status, msg): (StatusCode, String)) -> Self {
        JiraRouteError::Plain(status, msg)
    }
}

impl axum::response::IntoResponse for JiraRouteError {
    fn into_response(self) -> axum::response::Response {
        match self {
            JiraRouteError::CredentialInvalid { status } => {
                let body = serde_json::json!({
                    "code": "jira_credential_invalid",
                    "message": format!(
                        "Your saved Jira API token was rejected by Jira (HTTP {status}). \
                         Update it in Ticketing settings to continue."
                    ),
                });
                (StatusCode::UNAUTHORIZED, Json(body)).into_response()
            }
            JiraRouteError::Plain(status, msg) => (status, msg).into_response(),
        }
    }
}

/// Map a `TakutoError` from a per-user Jira REST call to a [`JiraRouteError`]:
/// `CredentialRejected` (401/403) → the modal code; the local
/// `TicketNotInConfiguredProjects` guard → plain `403`; everything else →
/// plain text at `fallback` (typically `502`).
pub fn map_jira_err(e: takuto_core::error::TakutoError, fallback: StatusCode) -> JiraRouteError {
    use takuto_core::jira::JiraError;
    match &e {
        takuto_core::error::TakutoError::Jira(JiraError::CredentialRejected { status }) => {
            JiraRouteError::CredentialInvalid { status: *status }
        }
        takuto_core::error::TakutoError::Jira(JiraError::TicketNotInConfiguredProjects {
            ..
        }) => JiraRouteError::Plain(StatusCode::FORBIDDEN, e.to_string()),
        _ => JiraRouteError::Plain(fallback, e.to_string()),
    }
}

/// Query parameters for the manual picker / preview endpoints. The picker
/// passes the repository (workspace name) the item is being added to so the
/// Jira project keys can be resolved per-user-per-repository.
#[derive(Debug, Deserialize)]
pub struct RepositoryQuery {
    pub repository: String,
}

/// Resolve the caller's per-repo polling settings for `repository`, requiring
/// at least one Jira project key. Returns 400 when no DB is attached, the
/// repository name is empty, or the caller has no keys configured for it.
async fn resolve_repo_settings(
    auth_state: &AuthState,
    user_id: &str,
    repository: &str,
) -> Result<takuto_core::db::user_repo_polling_settings::RepoPollingSettings, (StatusCode, String)>
{
    let repository = repository.trim();
    if repository.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "repository query parameter is required".to_string(),
        ));
    }
    let db = auth_state.db.as_ref().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "database unavailable".to_string(),
    ))?;
    let settings = user_repo_polling_settings::get(db.adapter(), user_id, repository)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .unwrap_or_default();
    if settings.project_keys.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "No Jira project keys configured for this repository".to_string(),
        ));
    }
    Ok(settings)
}

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

/// All **To Do** issues for the repository's configured projects (every issue
/// type), backlog order — for the manual-start picker. The `?repository=`
/// query param names the repository whose per-user Jira project keys to use.
pub async fn list_todo_tickets_manual(
    State(cfg): State<ConfigState>,
    State(engine): State<EngineState>,
    State(auth_state): State<AuthState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Query(query): Query<RepositoryQuery>,
) -> Result<Json<Vec<TodoTicketRow>>, JiraRouteError> {
    let settings = resolve_repo_settings(&auth_state, &auth.user_id, &query.repository).await?;
    let project_keys = settings.project_keys;
    let jql_filter = settings.jql_filter;

    let config = cfg.config.read().await;
    let repo_path = PathBuf::from(&config.git.repo_path);
    drop(config);

    // Prefer the CALLER's per-user Jira REST credential (the site/email/token
    // they pasted); fall back to the host-wide `acli` client only when none is
    // stored. This is why a user without `acli auth login` no longer hits
    // "acli list To Do tickets failed: unauthorized".
    let rest_cred = match auth_state.db.as_ref() {
        Some(db) => resolve_rest_credential(db, &auth.user_id).await,
        None => None,
    };
    let tickets = match rest_cred {
        Some(cred) => {
            JiraRestClient::new(auth_state.jira_http.clone(), cred)
                .list_todo_tickets_by_rank(&project_keys, &jql_filter)
                .await
        }
        None => {
            JiraClient::new(repo_path)
                .list_todo_tickets_by_rank(&project_keys, &jql_filter)
                .await
        }
    }
    .map_err(|e| map_jira_err(e, StatusCode::BAD_GATEWAY))?;

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
/// The `?repository=` query param names the repository whose per-user Jira
/// project keys gate which projects the ticket may belong to.
pub async fn get_ticket_preview(
    State(cfg): State<ConfigState>,
    State(auth_state): State<AuthState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Path(key): Path<String>,
    Query(query): Query<RepositoryQuery>,
) -> Result<Json<TicketDescriptionPreview>, JiraRouteError> {
    let settings = resolve_repo_settings(&auth_state, &auth.user_id, &query.repository).await?;
    let project_keys = settings.project_keys;

    let config = cfg.config.read().await;
    let repo_path = PathBuf::from(&config.git.repo_path);
    drop(config);

    // Prefer the CALLER's per-user Jira REST credential; fall back to `acli`.
    let rest_cred = match auth_state.db.as_ref() {
        Some(db) => resolve_rest_credential(db, &auth.user_id).await,
        None => None,
    };
    let preview = match rest_cred {
        Some(cred) => {
            JiraRestClient::new(auth_state.jira_http.clone(), cred)
                .get_ticket_description_preview(&key, &project_keys)
                .await
        }
        None => {
            JiraClient::new(repo_path)
                .get_ticket_description_preview(&key, &project_keys)
                .await
        }
    }
    .map_err(|e| map_jira_err(e, StatusCode::BAD_GATEWAY))?;

    Ok(Json(preview))
}

#[cfg(test)]
mod error_tests {
    use super::*;
    use axum::response::IntoResponse;
    use http_body_util::BodyExt;
    use takuto_core::error::TakutoError;
    use takuto_core::jira::JiraError;

    #[tokio::test]
    async fn credential_invalid_emits_401_json_envelope() {
        let resp = JiraRouteError::CredentialInvalid { status: 403 }.into_response();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["code"], "jira_credential_invalid");
        // message names the upstream status so the user knows what happened.
        assert!(json["message"].as_str().unwrap().contains("403"));
    }

    #[tokio::test]
    async fn plain_error_stays_plain_text() {
        let resp =
            JiraRouteError::Plain(StatusCode::BAD_GATEWAY, "acli boom".into()).into_response();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        // Not the JSON envelope — no `code` field (no false modal trigger).
        assert_eq!(&body[..], b"acli boom");
    }

    #[test]
    fn map_jira_err_classifies_each_case() {
        // REST 401/403 → modal code.
        assert!(matches!(
            map_jira_err(
                TakutoError::Jira(JiraError::CredentialRejected { status: 401 }),
                StatusCode::BAD_GATEWAY
            ),
            JiraRouteError::CredentialInvalid { status: 401 }
        ));
        // Local "ticket not in configured projects" guard → plain 403 (no code).
        assert!(matches!(
            map_jira_err(
                TakutoError::Jira(JiraError::TicketNotInConfiguredProjects { key: "X-1".into() }),
                StatusCode::BAD_GATEWAY
            ),
            JiraRouteError::Plain(StatusCode::FORBIDDEN, _)
        ));
        // Anything else (e.g. acli/no-token failure) → plain fallback (no code).
        assert!(matches!(
            map_jira_err(
                TakutoError::Jira(JiraError::ListTodoFailed { stderr: "x".into() }),
                StatusCode::BAD_GATEWAY
            ),
            JiraRouteError::Plain(StatusCode::BAD_GATEWAY, _)
        ));
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::server::build_router;
    use crate::state::AppState;
    use crate::test_helpers::{register_and_login, test_state_with_db};

    /// Seed per-repo Jira project keys for the logged-in `admin` user.
    async fn seed_keys(state: &AppState, repository: &str, keys: &[&str]) {
        let db = state.auth.db.as_ref().expect("db");
        let user_id = takuto_core::db::users::get_user_by_username(db.adapter(), "admin")
            .await
            .expect("query user")
            .expect("admin exists")
            .id;
        let settings = takuto_core::db::user_repo_polling_settings::RepoPollingSettings {
            project_keys: keys.iter().map(|s| s.to_string()).collect(),
            ..takuto_core::db::user_repo_polling_settings::RepoPollingSettings::default()
        };
        takuto_core::db::user_repo_polling_settings::set(
            db.adapter(),
            &user_id,
            repository,
            &settings,
        )
        .await
        .expect("seed settings");
    }

    #[tokio::test]
    async fn list_todo_tickets_returns_400_when_no_keys_for_repo() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        // Repository has no configured keys → 400.
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::get("/api/jira/todo-tickets-manual?repository=takuto-core")
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
            text.contains("No Jira project keys configured for this repository"),
            "expected per-repo keys error, got: {text}"
        );
    }

    #[tokio::test]
    async fn list_todo_tickets_returns_400_when_repository_param_missing() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        // Missing required `repository` query param → axum rejects with 400.
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
    }

    #[tokio::test]
    async fn get_ticket_preview_returns_403_when_project_not_in_repo_keys() {
        // Seed keys ["PROJ"] for repo "takuto-core" but request a ticket from
        // the "OTHER" project — the key's prefix is not in the repo's keys.
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;
        seed_keys(&state, "takuto-core", &["PROJ"]).await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::get("/api/jira/tickets/OTHER-123/preview?repository=takuto-core")
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

    #[tokio::test]
    async fn get_ticket_preview_returns_400_when_no_keys_for_repo() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::get("/api/jira/tickets/PROJ-1/preview?repository=takuto-core")
                    .header("Cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
