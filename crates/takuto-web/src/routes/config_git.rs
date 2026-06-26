// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! `PUT /api/config/git` — admin-only patch endpoint for the `[git]` section
//! (base branch + remote name).
//!
//! Mirrors `routes/config_jira.rs`: accepts `Json<serde_json::Value>` then
//! `from_value` so unknown fields map to **400** (not axum's default 422),
//! applies the patch under the `config.write()` lock, runs `config.validate()`
//! (→ 400), clones the snapshot, drops the lock, and persists outside the lock
//! via `ConfigWriter::write_config`.
//!
//! `repo_path` is intentionally NOT patchable here: it is a startup / workspace
//! field driven by the clone and workspace-switch flows, not the onboarding
//! wizard. Both patch fields are optional, but an all-null body is rejected so
//! a no-op PUT cannot masquerade as a successful save.

use axum::Json;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use serde::Deserialize;
use tracing::warn;

use crate::auth::AuthenticatedUser;
use crate::routes::admin::require_admin_for;
use crate::routes::config::{UpdateConfigResponse, stage_and_commit};
use crate::state::{AuthState, ConfigState};

// ---------------------------------------------------------------------------
// Request body — both fields optional, deny_unknown_fields.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutGitConfigRequest {
    /// Base branch worktrees are cut from (e.g. `main`). Must be non-empty.
    #[serde(default)]
    pub base_branch: Option<String>,
    /// Git remote name used for fetch / push (e.g. `origin`). Must be non-empty.
    /// `Config::validate` enforces non-empty independently.
    #[serde(default)]
    pub remote: Option<String>,
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// `PUT /api/config/git` — admin-only patch of the `[git]` section.
pub async fn put_git_config(
    State(auth_state): State<AuthState>,
    State(cfg_state): State<ConfigState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Json(raw): Json<serde_json::Value>,
) -> Result<Json<UpdateConfigResponse>, (StatusCode, String)> {
    require_admin_for(&auth_state, &auth)
        .await
        .map_err(|s| (s, String::new()))?;

    let patch: PutGitConfigRequest = serde_json::from_value(raw).map_err(|e| {
        let body = serde_json::json!({
            "error": "unknown_field_or_invalid_shape",
            "detail": e.to_string(),
        })
        .to_string();
        (StatusCode::BAD_REQUEST, body)
    })?;

    // At least one field must be present — reject an all-null patch.
    if patch.base_branch.is_none() && patch.remote.is_none() {
        return Err((
            StatusCode::BAD_REQUEST,
            "at least one of base_branch or remote must be provided".to_string(),
        ));
    }

    // Reject blank strings up front. `validate()` already guards `remote`, but
    // it has no non-empty check for `base_branch`, so we enforce both here for
    // a clear, symmetric error.
    if let Some(ref v) = patch.base_branch
        && v.trim().is_empty()
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "base_branch must be a non-empty branch name".to_string(),
        ));
    }
    if let Some(ref v) = patch.remote
        && v.trim().is_empty()
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "remote must be a non-empty remote name".to_string(),
        ));
    }

    let config_snapshot = stage_and_commit(&cfg_state.config, |config| {
        if let Some(v) = patch.base_branch {
            config.git.base_branch = v;
        }
        if let Some(v) = patch.remote {
            config.git.remote = v;
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
                    "[git] config patched in memory but disk write failed"
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
    async fn put_git_config_persists_fields() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::put("/api/config/git")
                    .header("Content-Type", "application/json")
                    .header("Origin", TEST_ORIGIN)
                    .header("Cookie", &cookie)
                    .body(Body::from(
                        r#"{"base_branch":"develop","remote":"upstream"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["git"]["base_branch"], "develop");
        assert_eq!(json["git"]["remote"], "upstream");

        let cfg = state.config.config.read().await;
        assert_eq!(cfg.git.base_branch, "develop");
        assert_eq!(cfg.git.remote, "upstream");
    }

    #[tokio::test]
    async fn put_git_config_partial_patch_leaves_other_field() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::put("/api/config/git")
                    .header("Content-Type", "application/json")
                    .header("Origin", TEST_ORIGIN)
                    .header("Cookie", &cookie)
                    .body(Body::from(r#"{"base_branch":"trunk"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let cfg = state.config.config.read().await;
        assert_eq!(cfg.git.base_branch, "trunk");
        // remote keeps its default.
        assert_eq!(cfg.git.remote, "origin");
    }

    #[tokio::test]
    async fn put_git_config_all_null_returns_400() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::put("/api/config/git")
                    .header("Content-Type", "application/json")
                    .header("Origin", TEST_ORIGIN)
                    .header("Cookie", &cookie)
                    .body(Body::from(r#"{}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn put_git_config_blank_base_branch_returns_400() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::put("/api/config/git")
                    .header("Content-Type", "application/json")
                    .header("Origin", TEST_ORIGIN)
                    .header("Cookie", &cookie)
                    .body(Body::from(r#"{"base_branch":"   "}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn put_git_config_blank_remote_returns_400() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::put("/api/config/git")
                    .header("Content-Type", "application/json")
                    .header("Origin", TEST_ORIGIN)
                    .header("Cookie", &cookie)
                    .body(Body::from(r#"{"remote":""}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn put_git_config_unknown_field_returns_400() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::put("/api/config/git")
                    .header("Content-Type", "application/json")
                    .header("Origin", TEST_ORIGIN)
                    .header("Cookie", &cookie)
                    .body(Body::from(r#"{"repo_path":"/x"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn put_git_config_non_admin_returns_403() {
        let state = test_state_with_db();
        let admin_cookie = register_and_login(&state).await;
        let user_cookie = create_and_login_user(&state, &admin_cookie).await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::put("/api/config/git")
                    .header("Content-Type", "application/json")
                    .header("Origin", TEST_ORIGIN)
                    .header("Cookie", &user_cookie)
                    .body(Body::from(r#"{"base_branch":"main"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }
}
