// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! `PUT /api/config/polling` — admin-only patch endpoint for the `[polling]`
//! section (auto-start flow, parallel-item caps, per-system filtering) plus a
//! top-level `item_types` that patches `[jira] item_types`.
//!
//! Mirrors `routes/config_agent.rs`. A new endpoint (not an extension of the
//! generic `PUT /api/config` allowlist) because the polling surface is richer
//! than the strict four-field `RuntimeDashboardConfigPatch` and carries
//! `jira.item_types`, which that allowlist cannot express. Unlike the agent
//! endpoint, this does no `EngineState` status refresh or WS broadcast — the
//! pollers read the new config live on their next cycle.

use std::sync::atomic::Ordering;

use axum::Json;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use serde::Deserialize;
use tracing::warn;

use crate::auth::AuthenticatedUser;
use crate::routes::admin::require_admin_for;
use crate::routes::config::UpdateConfigResponse;
use crate::state::{AuthState, ConfigState, EngineState};

// ---------------------------------------------------------------------------
// Request bodies — every field optional, deny_unknown_fields throughout.
// Vecs replace wholesale when present.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutPollingConfigRequest {
    #[serde(default)]
    pub auto_start_flow: Option<String>,
    #[serde(default)]
    pub max_parallel_items: Option<u32>,
    #[serde(default)]
    pub max_parallel_per_user: Option<bool>,
    #[serde(default)]
    pub jira: Option<PollingJiraPatch>,
    #[serde(default)]
    pub github: Option<PollingGitHubPatch>,
    /// Patches `[jira] item_types` (the generic `PUT /api/config` allowlist
    /// cannot carry it). Replaces the list wholesale when present.
    #[serde(default)]
    pub item_types: Option<Vec<String>>,
    /// Patches `[general] poll_interval_secs` — how often the poller runs.
    /// `Config::validate` enforces the `>= 10` floor.
    #[serde(default)]
    pub poll_interval_secs: Option<u64>,
    /// Enable/disable item polling. Patches `[general] auto_polling` (persisted)
    /// and flips the live `polling_paused` flag so it takes effect immediately.
    #[serde(default)]
    pub auto_polling: Option<bool>,
    /// Patches `[general] max_concurrent_manual_workflows` (`0` = no limit).
    #[serde(default)]
    pub max_concurrent_manual_workflows: Option<u32>,
    /// Patches `[general] pr_merge_poll_interval_secs` (`0` disables PR-merge polling).
    #[serde(default)]
    pub pr_merge_poll_interval_secs: Option<u64>,
    /// Patches `[general] generate_report`.
    #[serde(default)]
    pub generate_report: Option<bool>,
    /// Patches `[general] work_item_log_retention_days` (`0` = keep forever).
    #[serde(default)]
    pub work_item_log_retention_days: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PollingJiraPatch {
    #[serde(default)]
    pub summary_keywords: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PollingGitHubPatch {
    #[serde(default)]
    pub labels: Option<Vec<String>>,
    #[serde(default)]
    pub title_keywords: Option<Vec<String>>,
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// `PUT /api/config/polling` — admin-only patch of the `[polling]` section.
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
    State(engine_state): State<EngineState>,
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

        if let Some(v) = patch.auto_start_flow {
            config.polling.auto_start_flow = v;
        }
        if let Some(v) = patch.max_parallel_items {
            config.polling.max_parallel_items = v;
        }
        if let Some(v) = patch.max_parallel_per_user {
            config.polling.max_parallel_per_user = v;
        }
        if let Some(jira) = patch.jira
            && let Some(kw) = jira.summary_keywords
        {
            config.polling.jira.summary_keywords = kw;
        }
        if let Some(github) = patch.github {
            if let Some(labels) = github.labels {
                config.polling.github.labels = labels;
            }
            if let Some(kw) = github.title_keywords {
                config.polling.github.title_keywords = kw;
            }
        }
        if let Some(item_types) = patch.item_types {
            config.jira.item_types = item_types;
        }
        if let Some(v) = patch.poll_interval_secs {
            config.general.poll_interval_secs = v;
        }
        if let Some(v) = patch.auto_polling {
            config.general.auto_polling = v;
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

    // Apply the enable/disable to the live poller immediately — the poller
    // reads `polling_paused` each cycle, so this takes effect without a restart
    // (the persisted `auto_polling` covers the next start).
    if let Some(enabled) = patch.auto_polling {
        engine_state
            .polling_paused
            .store(!enabled, Ordering::Relaxed);
    }

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
    use std::sync::atomic::Ordering;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
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
    async fn put_polling_replaces_vecs_on_present() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        // Seed a pre-existing keyword so we can prove wholesale replacement.
        {
            let mut cfg = state.config.config.write().await;
            cfg.polling.jira.summary_keywords = vec!["old".to_string()];
        }

        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::put("/api/config/polling")
                    .header("Content-Type", "application/json")
                    .header("Origin", TEST_ORIGIN)
                    .header("Cookie", &cookie)
                    .body(Body::from(
                        r#"{"auto_start_flow":"implement-ticket","max_parallel_items":4,"max_parallel_per_user":true,"jira":{"summary_keywords":["crash","urgent"]},"github":{"labels":["bug"],"title_keywords":["panic"]}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["polling"]["auto_start_flow"], "implement-ticket");
        assert_eq!(json["polling"]["max_parallel_items"], 4);
        assert_eq!(json["polling"]["max_parallel_per_user"], true);

        let cfg = state.config.config.read().await;
        assert_eq!(
            cfg.polling.jira.summary_keywords,
            vec!["crash".to_string(), "urgent".to_string()]
        );
        assert_eq!(cfg.polling.github.labels, vec!["bug".to_string()]);
        assert_eq!(cfg.polling.github.title_keywords, vec!["panic".to_string()]);
    }

    #[tokio::test]
    async fn put_polling_item_types_patches_jira_item_types() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::put("/api/config/polling")
                    .header("Content-Type", "application/json")
                    .header("Origin", TEST_ORIGIN)
                    .header("Cookie", &cookie)
                    .body(Body::from(r#"{"item_types":["Story","Epic"]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let cfg = state.config.config.read().await;
        assert_eq!(
            cfg.jira.item_types,
            vec!["Story".to_string(), "Epic".to_string()]
        );
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
    async fn put_polling_auto_polling_toggles_config_and_live_flag() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;
        // Live flag starts enabled (not paused) in the test harness.
        assert!(!state.engine.polling_paused.load(Ordering::Relaxed));

        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::put("/api/config/polling")
                    .header("Content-Type", "application/json")
                    .header("Origin", TEST_ORIGIN)
                    .header("Cookie", &cookie)
                    .body(Body::from(r#"{"auto_polling":false}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Persisted to config and applied to the live poller immediately.
        assert!(!state.config.config.read().await.general.auto_polling);
        assert!(state.engine.polling_paused.load(Ordering::Relaxed));
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

    #[tokio::test]
    async fn put_polling_blank_keyword_returns_400() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::put("/api/config/polling")
                    .header("Content-Type", "application/json")
                    .header("Origin", TEST_ORIGIN)
                    .header("Cookie", &cookie)
                    .body(Body::from(r#"{"jira":{"summary_keywords":["ok","   "]}}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
