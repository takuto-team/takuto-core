// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! `PUT /api/config/polling` — admin-only patch endpoint for the
//! deployment-global polling limits in `[general]`.
//!
//! Per-repository polling policy (project keys, item types, filters,
//! auto-start flow, per-repo parallel cap, Jira context) is **per-user,
//! per-repository** now — see `/api/me/polling-settings`. This endpoint keeps
//! only the deployment-wide knobs: the shared poll-loop cadence
//! (`poll_interval_secs`), the cross-repo per-user concurrency policy
//! (`max_parallel_per_user`), and the manual / PR-merge / report / log-retention
//! limits.

use axum::Json;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use serde::Deserialize;
use tracing::warn;

use crate::auth::AuthenticatedUser;
use crate::routes::admin::require_admin_for;
use crate::routes::config::UpdateConfigResponse;
use crate::state::{AuthState, ConfigState};

// ---------------------------------------------------------------------------
// Request bodies — every field optional, deny_unknown_fields throughout.
// Vecs replace wholesale when present.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutPollingConfigRequest {
    /// Patches `[general] poll_interval_secs` — the shared poll loop cadence.
    /// `Config::validate` enforces the `>= 10` floor.
    #[serde(default)]
    pub poll_interval_secs: Option<u64>,
    /// Patches `[general] max_parallel_per_user` — whether the per-repo
    /// `max_parallel_items` cap is scoped per workflow owner.
    #[serde(default)]
    pub max_parallel_per_user: Option<bool>,
    /// Patches `[general] max_concurrent_manual_workflows` (`0` = no limit).
    #[serde(default)]
    pub max_concurrent_manual_workflows: Option<u32>,
    /// Patches `[general] pr_merge_poll_interval_secs` (`0` disables PR-merge polling).
    #[serde(default)]
    pub pr_merge_poll_interval_secs: Option<u64>,
    /// Patches `[general] generate_report` (deployment default).
    #[serde(default)]
    pub generate_report: Option<bool>,
    /// Patches `[general] work_item_log_retention_days` (`0` = keep forever).
    #[serde(default)]
    pub work_item_log_retention_days: Option<u32>,
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// `PUT /api/config/polling` — admin-only patch of the deployment-global
/// polling limits in `[general]`.
///
/// 1. `require_admin_for` — 403 for non-admin.
/// 2. Accept `Json<serde_json::Value>` then `from_value` so unknown fields map
///    to **400** (not axum's default 422).
/// 3. Apply the patch under the `config.write()` lock; `config.validate()` → 400.
/// 4. Clone the snapshot, drop the lock, persist outside the lock via
///    `ConfigWriter::write_config`.
pub async fn put_polling_config(
    State(auth_state): State<AuthState>,
    State(cfg_state): State<ConfigState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Json(raw): Json<serde_json::Value>,
) -> Result<Json<UpdateConfigResponse>, (StatusCode, String)> {
    require_admin_for(&auth_state, &auth)
        .await
        .map_err(|s| (s, String::new()))?;

    let patch: PutPollingConfigRequest = serde_json::from_value(raw).map_err(|e| {
        let body = serde_json::json!({
            "error": "unknown_field_or_invalid_shape",
            "detail": e.to_string(),
        })
        .to_string();
        (StatusCode::BAD_REQUEST, body)
    })?;

    let config_snapshot = {
        let mut config = cfg_state.config.write().await;

        if let Some(v) = patch.poll_interval_secs {
            config.general.poll_interval_secs = v;
        }
        if let Some(v) = patch.max_parallel_per_user {
            config.general.max_parallel_per_user = v;
        }
        if let Some(v) = patch.max_concurrent_manual_workflows {
            config.general.max_concurrent_manual_workflows = v;
        }
        if let Some(v) = patch.pr_merge_poll_interval_secs {
            config.general.pr_merge_poll_interval_secs = v;
        }
        if let Some(v) = patch.generate_report {
            config.general.generate_report = v;
        }
        if let Some(v) = patch.work_item_log_retention_days {
            config.general.work_item_log_retention_days = v;
        }

        config
            .validate()
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

        config.clone()
    };

    let (persisted, persist_warning) = if let Some(ref writer) = cfg_state.config_writer {
        match writer.write_config(&config_snapshot) {
            Ok(()) => (true, None),
            Err(e) => {
                warn!(
                    error = %e,
                    "[polling] config patched in memory but disk write failed"
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
    use tower::ServiceExt;

    use crate::server::build_router;
    use crate::state::AppState;
    use crate::test_helpers::{TEST_ORIGIN, register_and_login, test_state_with_db};

    /// Register a non-admin user (created by the admin) and log in, returning
    /// the session cookie. Requires an existing admin cookie.
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
        assert_eq!(
            resp.status(),
            StatusCode::CREATED,
            "create user should succeed"
        );

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
    async fn put_polling_unknown_field_returns_400() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::put("/api/config/polling")
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
    async fn put_polling_non_admin_returns_403() {
        let state = test_state_with_db();
        let admin_cookie = register_and_login(&state).await;
        let user_cookie = create_and_login_user(&state, &admin_cookie).await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::put("/api/config/polling")
                    .header("Content-Type", "application/json")
                    .header("Origin", TEST_ORIGIN)
                    .header("Cookie", &user_cookie)
                    .body(Body::from(r#"{"max_parallel_items":3}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn put_polling_max_parallel_per_user_patches_general() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::put("/api/config/polling")
                    .header("Content-Type", "application/json")
                    .header("Origin", TEST_ORIGIN)
                    .header("Cookie", &cookie)
                    .body(Body::from(r#"{"max_parallel_per_user":true}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(
            state
                .config
                .config
                .read()
                .await
                .general
                .max_parallel_per_user
        );
    }

    /// Per-repo fields (moved to /api/me/polling-settings) are no longer
    /// accepted by the global endpoint — `deny_unknown_fields` → 400.
    #[tokio::test]
    async fn put_polling_rejects_moved_per_repo_fields() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        for body in [
            r#"{"auto_start_flow":"x"}"#,
            r#"{"max_parallel_items":4}"#,
            r#"{"item_types":["Story"]}"#,
            r#"{"jira":{"summary_keywords":["x"]}}"#,
            r#"{"auto_polling":false}"#,
        ] {
            let app = build_router(state.clone());
            let resp = app
                .oneshot(
                    Request::put("/api/config/polling")
                        .header("Content-Type", "application/json")
                        .header("Origin", TEST_ORIGIN)
                        .header("Cookie", &cookie)
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::BAD_REQUEST,
                "moved field must be rejected: {body}"
            );
        }
    }

    #[tokio::test]
    async fn put_polling_poll_interval_patches_general() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::put("/api/config/polling")
                    .header("Content-Type", "application/json")
                    .header("Origin", TEST_ORIGIN)
                    .header("Cookie", &cookie)
                    .body(Body::from(r#"{"poll_interval_secs":120}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let cfg = state.config.config.read().await;
        assert_eq!(cfg.general.poll_interval_secs, 120);
    }

    #[tokio::test]
    async fn put_polling_poll_interval_below_floor_returns_400() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::put("/api/config/polling")
                    .header("Content-Type", "application/json")
                    .header("Origin", TEST_ORIGIN)
                    .header("Cookie", &cookie)
                    .body(Body::from(r#"{"poll_interval_secs":5}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn put_polling_general_limits_patch_general() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::put("/api/config/polling")
                    .header("Content-Type", "application/json")
                    .header("Origin", TEST_ORIGIN)
                    .header("Cookie", &cookie)
                    .body(Body::from(
                        r#"{"max_concurrent_manual_workflows":3,"pr_merge_poll_interval_secs":120,"generate_report":true,"work_item_log_retention_days":14}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let cfg = state.config.config.read().await;
        assert_eq!(cfg.general.max_concurrent_manual_workflows, 3);
        assert_eq!(cfg.general.pr_merge_poll_interval_secs, 120);
        assert!(cfg.general.generate_report);
        assert_eq!(cfg.general.work_item_log_retention_days, 14);
    }
}
