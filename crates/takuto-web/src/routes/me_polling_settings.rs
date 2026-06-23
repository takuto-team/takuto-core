// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Per-user REST endpoints for per-repository polling settings.
//!
//! Mounted under `/api/me/polling-settings/*` (no admin gate). Every handler
//! reads `Extension<AuthenticatedUser>` and operates on `auth.user_id` ONLY —
//! the URL never carries a `user_id` and admins have no special path to
//! another user's data.
//!
//! | Method | Path                                       |
//! |--------|--------------------------------------------|
//! | GET    | `/api/me/polling-settings`                 |
//! | GET    | `/api/me/polling-settings/{workspace}`     |
//! | PUT    | `/api/me/polling-settings/{workspace}`     |
//! | DELETE | `/api/me/polling-settings/{workspace}`     |
//!
//! PUT body is the [`RepoPollingSettings`] object. `project_keys` are validated
//! (non-empty ASCII-alphanumeric, max 50); `poll_interval_secs` floored at 10.

use axum::Json;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use serde::Serialize;
use ts_rs::TS;

use takuto_core::db::user_repo_polling_settings::{
    self, RepoPollingSettings, UserRepoPollingSettingsRow,
};

use crate::auth::AuthenticatedUser;
use crate::state::AuthState;

/// Hard ceiling on the number of project keys per workspace.
const MAX_KEYS: usize = 50;

/// One per-workspace polling-settings row, as returned to the dashboard.
#[derive(Debug, Serialize, TS)]
#[ts(
    rename = "RepoPollingSettingsRow",
    export_to = "RepoPollingSettingsRow.ts"
)]
pub struct RepoPollingSettingsEntry {
    pub workspace_name: String,
    pub settings: RepoPollingSettings,
    pub updated_at: i64,
}

impl From<UserRepoPollingSettingsRow> for RepoPollingSettingsEntry {
    fn from(row: UserRepoPollingSettingsRow) -> Self {
        RepoPollingSettingsEntry {
            workspace_name: row.workspace_name,
            settings: row.settings,
            updated_at: row.updated_at,
        }
    }
}

fn validate_workspace_name(name: &str) -> Result<(), (StatusCode, String)> {
    if name.is_empty()
        || name.contains('/')
        || name.contains("..")
        || name.starts_with('.')
        || name.contains('\0')
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "Invalid workspace name".to_string(),
        ));
    }
    Ok(())
}

/// Validate the settings payload. Project keys use the same predicate as the
/// old global field (non-empty ASCII-alphanumeric); the interval is floored.
fn validate_settings(s: &RepoPollingSettings) -> Result<(), (StatusCode, String)> {
    if s.project_keys.len() > MAX_KEYS {
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "Too many project keys: {} > {MAX_KEYS}",
                s.project_keys.len()
            ),
        ));
    }
    for key in &s.project_keys {
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("invalid key '{key}': must be non-empty alphanumeric"),
            ));
        }
    }
    Ok(())
}

fn db_error(e: takuto_core::error::TakutoError) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

fn require_db(auth_state: &AuthState) -> Result<takuto_core::db::Database, (StatusCode, String)> {
    auth_state.db.clone().ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        "database unavailable".into(),
    ))
}

/// `GET /api/me/polling-settings` — the caller's rows.
pub async fn list_my_rows(
    State(auth_state): State<AuthState>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<Json<Vec<RepoPollingSettingsEntry>>, (StatusCode, String)> {
    let db = require_db(&auth_state)?;
    let rows = user_repo_polling_settings::list_for_user(db.adapter(), &auth.user_id)
        .await
        .map_err(db_error)?;
    let mut entries: Vec<RepoPollingSettingsEntry> = rows
        .into_iter()
        .map(RepoPollingSettingsEntry::from)
        .collect();
    entries.sort_by(|a, b| a.workspace_name.cmp(&b.workspace_name));
    Ok(Json(entries))
}

/// `GET /api/me/polling-settings/{workspace}` — the caller's row, or 404.
pub async fn get_my_row(
    State(auth_state): State<AuthState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Path(workspace): Path<String>,
) -> Result<Json<RepoPollingSettingsEntry>, (StatusCode, String)> {
    validate_workspace_name(&workspace)?;
    let db = require_db(&auth_state)?;
    let rows = user_repo_polling_settings::list_for_user(db.adapter(), &auth.user_id)
        .await
        .map_err(db_error)?;
    match rows.into_iter().find(|r| r.workspace_name == workspace) {
        Some(r) => Ok(Json(r.into())),
        None => Err((StatusCode::NOT_FOUND, "No polling settings set".to_string())),
    }
}

/// `PUT /api/me/polling-settings/{workspace}` — upsert the caller's settings.
pub async fn put_my_row(
    State(auth_state): State<AuthState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Path(workspace): Path<String>,
    Json(settings): Json<RepoPollingSettings>,
) -> Result<Json<RepoPollingSettingsEntry>, (StatusCode, String)> {
    validate_workspace_name(&workspace)?;
    validate_settings(&settings)?;
    let db = require_db(&auth_state)?;

    user_repo_polling_settings::set(db.adapter(), &auth.user_id, &workspace, &settings)
        .await
        .map_err(db_error)?;

    let row = user_repo_polling_settings::list_for_user(db.adapter(), &auth.user_id)
        .await
        .map_err(db_error)?
        .into_iter()
        .find(|r| r.workspace_name == workspace)
        .ok_or_else(|| db_error(takuto_core::db::DbError::RowDisappearedAfterUpsert.into()))?;

    tracing::info!(
        user_id = %auth.user_id,
        workspace_name = %workspace,
        action = "set",
        auto_polling = row.settings.auto_polling,
        key_count = row.settings.project_keys.len(),
        "user_repo_polling_settings changed"
    );

    Ok(Json(row.into()))
}

/// `DELETE /api/me/polling-settings/{workspace}` — remove the caller's row.
pub async fn delete_my_row(
    State(auth_state): State<AuthState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Path(workspace): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    validate_workspace_name(&workspace)?;
    let db = require_db(&auth_state)?;
    let deleted = user_repo_polling_settings::delete(db.adapter(), &auth.user_id, &workspace)
        .await
        .map_err(db_error)?;
    if !deleted {
        return Err((
            StatusCode::NOT_FOUND,
            "No polling settings to delete".to_string(),
        ));
    }
    tracing::info!(
        user_id = %auth.user_id,
        workspace_name = %workspace,
        action = "delete",
        "user_repo_polling_settings changed"
    );
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod ts_bindings {
    use super::*;
    use ts_rs::TS;

    #[test]
    fn export_repo_polling_settings_row() {
        let out = crate::ts_bindings::generated_dir();
        std::fs::create_dir_all(&out).expect("create generated dir");
        RepoPollingSettingsEntry::export_all_to(&out).expect("export RepoPollingSettingsRow");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_workspace_name_rejects_traversal() {
        assert!(validate_workspace_name("").is_err());
        assert!(validate_workspace_name("a/b").is_err());
        assert!(validate_workspace_name("..").is_err());
        assert!(validate_workspace_name(".hidden").is_err());
        assert!(validate_workspace_name("ok-name").is_ok());
    }

    #[test]
    fn validate_settings_keys() {
        let mut s = RepoPollingSettings {
            project_keys: vec!["PROJ".into(), "OPS2".into()],
            ..RepoPollingSettings::default()
        };
        assert!(validate_settings(&s).is_ok());

        s.project_keys = vec!["bad key".into()];
        assert!(validate_settings(&s).is_err());

        s.project_keys = (0..51).map(|i| format!("PROJ{i}")).collect();
        assert!(validate_settings(&s).is_err());
    }
}

#[cfg(test)]
mod route_tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::server::build_router;
    use crate::test_helpers::{TEST_ORIGIN, register_and_login, test_state_with_db};

    #[tokio::test]
    async fn full_crud_roundtrip() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        // Empty initially.
        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::get("/api/me/polling-settings")
                    .header("Cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // PUT settings.
        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::put("/api/me/polling-settings/takuto-core")
                    .header("Content-Type", "application/json")
                    .header("Origin", TEST_ORIGIN)
                    .header("Cookie", &cookie)
                    .body(Body::from(
                        r#"{"auto_polling":true,"poll_interval_secs":30,"project_keys":["PROJ","OPS"]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["workspace_name"], "takuto-core");
        assert_eq!(json["settings"]["auto_polling"], true);
        assert_eq!(
            json["settings"]["project_keys"],
            serde_json::json!(["PROJ", "OPS"])
        );
        // Defaults filled in for unspecified fields.
        assert_eq!(
            json["settings"]["item_types"],
            serde_json::json!(["Task", "Bug"])
        );
        assert_eq!(json["settings"]["done_status"], "Done");

        // GET single.
        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::get("/api/me/polling-settings/takuto-core")
                    .header("Cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // DELETE.
        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::delete("/api/me/polling-settings/takuto-core")
                    .header("Origin", TEST_ORIGIN)
                    .header("Cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        // GET now 404.
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::get("/api/me/polling-settings/takuto-core")
                    .header("Cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn put_rejects_invalid_key() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::put("/api/me/polling-settings/takuto-core")
                    .header("Content-Type", "application/json")
                    .header("Origin", TEST_ORIGIN)
                    .header("Cookie", &cookie)
                    .body(Body::from(r#"{"project_keys":["bad key!"]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn endpoints_require_auth() {
        let state = test_state_with_db();
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::get("/api/me/polling-settings")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}
