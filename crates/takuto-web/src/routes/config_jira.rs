// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! `PUT /api/config/jira` — admin-only patch endpoint for the `[jira]` section
//! (filters and prompt-context policy).
//!
//! Mirrors `routes/config_polling.rs`: accepts `Json<serde_json::Value>` then
//! `from_value` so unknown fields map to **400** (not axum's default 422),
//! applies the patch under the `config.write()` lock, runs `config.validate()`
//! (→ 400), clones the snapshot, drops the lock, and persists outside the lock
//! via `ConfigWriter::write_config`.

use axum::Json;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use serde::Deserialize;
use tracing::warn;

use takuto_core::config::LinkedItemsPromptMode;

use crate::auth::AuthenticatedUser;
use crate::routes::admin::require_admin_for;
use crate::routes::config::{UpdateConfigResponse, stage_and_commit};
use crate::state::{AuthState, ConfigState};

// ---------------------------------------------------------------------------
// Request body — every field optional, deny_unknown_fields. Vecs replace
// wholesale when present.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutJiraConfigRequest {
    /// How linked issues appear in agent prompts. Deserializes the snake_case
    /// enum (`full` | `summary_only` | `omit`).
    #[serde(default)]
    pub linked_items_in_prompt: Option<LinkedItemsPromptMode>,
    /// Max UTF-8 bytes for the primary ticket description in prompts (`0` = unlimited).
    #[serde(default)]
    pub ticket_context_max_description_bytes: Option<usize>,
    /// Max UTF-8 bytes per linked issue description when mode is `full` (`0` = unlimited).
    #[serde(default)]
    pub linked_issue_description_max_bytes: Option<usize>,
    /// Jira transition target for **Mark as Done**. `Config::validate` enforces
    /// non-empty.
    #[serde(default)]
    pub done_status: Option<String>,
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// `PUT /api/config/jira` — admin-only patch of the `[jira]` section.
pub async fn put_jira_config(
    State(auth_state): State<AuthState>,
    State(cfg_state): State<ConfigState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Json(raw): Json<serde_json::Value>,
) -> Result<Json<UpdateConfigResponse>, (StatusCode, String)> {
    require_admin_for(&auth_state, &auth)
        .await
        .map_err(|s| (s, String::new()))?;

    let patch: PutJiraConfigRequest = serde_json::from_value(raw).map_err(|e| {
        let body = serde_json::json!({
            "error": "unknown_field_or_invalid_shape",
            "detail": e.to_string(),
        })
        .to_string();
        (StatusCode::BAD_REQUEST, body)
    })?;

    let config_snapshot = stage_and_commit(&cfg_state.config, |config| {
        if let Some(v) = patch.linked_items_in_prompt {
            config.jira.linked_items_in_prompt = v;
        }
        if let Some(v) = patch.ticket_context_max_description_bytes {
            config.jira.ticket_context_max_description_bytes = v;
        }
        if let Some(v) = patch.linked_issue_description_max_bytes {
            config.jira.linked_issue_description_max_bytes = v;
        }
        if let Some(v) = patch.done_status {
            config.jira.done_status = v;
        }
        Ok(())
    })
    .await?;

    let (persisted, persist_warning) = if let Some(ref writer) = cfg_state.config_writer {
        match writer.write_config(&config_snapshot) {
            Ok(()) => (true, None),
            Err(e) => {
                warn!(
                    error = %e,
                    "[jira] config patched in memory but disk write failed"
                );
                (false, Some(e.to_string()))
            }
        }
    } else {
        (false, None)
    };

    Ok(Json(UpdateConfigResponse {
        config: config_snapshot.redacted_for_api_clone(),
        persisted,
        persist_warning,
    }))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::server::build_router;
    use crate::state::AppState;
    use crate::test_helpers::{TEST_ORIGIN, register_and_login, test_state_with_db};

    async fn create_and_login_user(state: &AppState, admin_cookie: &str) -> String {
        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::post("/api/users")
                    .header("Content-Type", "application/json")
                    .header("Origin", TEST_ORIGIN)
                    .header("Cookie", admin_cookie)
                    .body(Body::from(
                        r#"{"username":"viewer","password":"viewerpass1234"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        let app = build_router(state.clone());
        let login = app
            .oneshot(
                Request::post("/api/auth/login")
                    .header("Content-Type", "application/json")
                    .header("Origin", TEST_ORIGIN)
                    .body(Body::from(
                        r#"{"username":"viewer","password":"viewerpass1234"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(login.status(), StatusCode::NO_CONTENT);
        let set_cookie = login
            .headers()
            .get("set-cookie")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        set_cookie.split(';').next().unwrap().trim().to_string()
    }

    #[tokio::test]
    async fn put_jira_config_persists_fields() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::put("/api/config/jira")
                    .header("Content-Type", "application/json")
                    .header("Origin", TEST_ORIGIN)
                    .header("Cookie", &cookie)
                    .body(Body::from(
                        r#"{"linked_items_in_prompt":"summary_only","ticket_context_max_description_bytes":4096,"linked_issue_description_max_bytes":1024,"done_status":"Closed"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["jira"]["linked_items_in_prompt"], "summary_only");
        assert_eq!(json["jira"]["done_status"], "Closed");

        let cfg = state.config.config.read().await;
        assert_eq!(cfg.jira.ticket_context_max_description_bytes, 4096);
        assert_eq!(cfg.jira.linked_issue_description_max_bytes, 1024);
        assert_eq!(cfg.jira.done_status, "Closed");
    }

    #[tokio::test]
    async fn put_jira_config_unknown_field_returns_400() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::put("/api/config/jira")
                    .header("Content-Type", "application/json")
                    .header("Origin", TEST_ORIGIN)
                    .header("Cookie", &cookie)
                    .body(Body::from(r#"{"bogus":true}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn put_jira_config_rejects_legacy_project_keys_field() {
        // `project_keys` was removed from the [jira] config section (keys are
        // now per-user-per-repository). The request body uses
        // `deny_unknown_fields`, so a client still sending it gets a 400.
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::put("/api/config/jira")
                    .header("Content-Type", "application/json")
                    .header("Origin", TEST_ORIGIN)
                    .header("Cookie", &cookie)
                    .body(Body::from(r#"{"project_keys":["PROJ"]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn put_jira_config_blank_done_status_returns_400() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::put("/api/config/jira")
                    .header("Content-Type", "application/json")
                    .header("Origin", TEST_ORIGIN)
                    .header("Cookie", &cookie)
                    .body(Body::from(r#"{"done_status":"   "}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn put_jira_config_non_admin_returns_403() {
        let state = test_state_with_db();
        let admin_cookie = register_and_login(&state).await;
        let user_cookie = create_and_login_user(&state, &admin_cookie).await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::put("/api/config/jira")
                    .header("Content-Type", "application/json")
                    .header("Origin", TEST_ORIGIN)
                    .header("Cookie", &user_cookie)
                    .body(Body::from(r#"{"done_status":"Done"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }
}
