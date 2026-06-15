// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::sync::atomic::Ordering;

use axum::Json;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use serde::Serialize;

use takuto_core::config::{Config, RuntimeDashboardConfigPatch};
use takuto_core::config_watcher::reload_config_from_disk;

use crate::auth::AuthenticatedUser;
use crate::routes::admin::require_admin_for;
use crate::state::{AuthState, ConfigState};

/// Wraps the redacted config with extra runtime flags that are not in `config.toml`.
#[derive(Serialize)]
pub struct ConfigResponse {
    #[serde(flatten)]
    pub config: Config,
    /// `true` when acli (Jira) is authenticated.
    pub jira_available: bool,
    /// Ticketing system in use: `"jira"`, `"github"`, or `"none"`.
    pub ticketing_system: String,
    /// `true` when a GitHub App is fully configured (`[github]` section has all required fields).
    pub github_app_configured: bool,
    /// Non-empty when preflight failed at startup (e.g. GitHub CLI not authenticated).
    /// The UI shows a blocking error banner when this is set.
    pub preflight_error: Option<String>,
    /// `true` when the config file is writable (ConfigWriter is available).
    pub config_writable: bool,
    /// `true` when a git repository exists at the configured `git.repo_path`.
    pub repo_exists: bool,
    /// Short name of the currently cloned repository (directory name under `/workspaces/`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_name: Option<String>,
    /// GitHub HTML URL of the currently cloned repository (from `git remote get-url origin`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_html_url: Option<String>,
    /// Display name of the connected GitHub App (e.g. `"sous-coder"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub github_app_name: Option<String>,
}

pub async fn get_version() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "version": takuto_core::VERSION.trim()
    }))
}

pub async fn get_config(State(cfg): State<ConfigState>) -> Json<ConfigResponse> {
    // Clone needed values and release the read lock before any filesystem I/O.
    let (config_clone, active_repo_path_str, github_configured, app_name_raw, ticketing_system) = {
        let config = cfg.config.read().await;
        let path = config.git.repo_path.clone();
        let gh_configured = config.github.is_configured();
        let app_name = if gh_configured && !config.github.app_name.is_empty() {
            Some(config.github.app_name.clone())
        } else {
            None
        };
        // Read ticketing_system from the LIVE config (not the startup
        // `ConfigState` snapshot) so a `PUT /api/config` change to
        // `[general] ticketing_system` is reflected on the next GET.
        let ticketing_system = config.general.ticketing_system.to_string();
        (
            config.redacted_for_api_clone(),
            path,
            gh_configured,
            app_name,
            ticketing_system,
        )
    }; // read lock dropped here

    let active_repo_path = std::path::PathBuf::from(&active_repo_path_str);
    let repo_exists = active_repo_path.join(".git").exists();
    let repo_name = if repo_exists {
        active_repo_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
    } else {
        None
    };
    let repo_html_url = if repo_exists {
        super::repos::read_git_remote_url(&active_repo_path)
    } else {
        None
    };

    Json(ConfigResponse {
        github_app_configured: github_configured,
        config: config_clone,
        jira_available: cfg.jira_available.load(Ordering::Relaxed),
        ticketing_system,
        preflight_error: cfg.preflight_error.clone(),
        config_writable: cfg.config_writer.is_some(),
        repo_exists,
        repo_name,
        repo_html_url,
        github_app_name: app_name_raw,
    })
}

/// Response for config update operations.
#[derive(Serialize)]
pub struct UpdateConfigResponse {
    #[serde(flatten)]
    pub config: Config,
    /// When `true`, the change was also persisted to disk and will survive
    /// restarts. When `false`, the change is active in memory but a disk
    /// write was not possible (e.g., config path is read-only).
    pub persisted: bool,
    /// Non-empty when the in-memory patch succeeded but the disk write failed.
    /// Contains the error message from the write attempt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub persist_warning: Option<String>,
}

pub async fn update_config(
    State(auth_state): State<AuthState>,
    State(cfg): State<ConfigState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Json(patch): Json<RuntimeDashboardConfigPatch>,
) -> Result<Json<UpdateConfigResponse>, (StatusCode, String)> {
    require_admin_for(&auth_state, &auth)
        .await
        .map_err(|s| (s, String::new()))?;
    // Apply patch under write lock, then clone and release.
    let config_snapshot = {
        let mut config = cfg.config.write().await;
        config
            .apply_runtime_dashboard_patch(patch)
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        config.clone()
    }; // write lock dropped here

    // Persist to disk OUTSIDE the lock — blocking I/O no longer holds the
    // write guard, so concurrent readers are not stalled.
    let (persisted, persist_warning) = if let Some(ref writer) = cfg.config_writer {
        match writer.write_config(&config_snapshot) {
            Ok(()) => (true, None),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Config patched in memory but disk write failed"
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

/// `POST /api/config/reload` — reload config from disk immediately.
///
/// Reads the config file, parses, validates, and replaces the in-memory
/// config. Returns the new config on success or a `400` with the error.
pub async fn reload_config(
    State(auth_state): State<AuthState>,
    State(cfg): State<ConfigState>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<Json<Config>, (StatusCode, String)> {
    require_admin_for(&auth_state, &auth)
        .await
        .map_err(|s| (s, String::new()))?;
    reload_config_from_disk(&cfg.config_path, &cfg.config)
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                format!("Config reload failed: {e}"),
            )
        })?;

    let config = cfg.config.read().await;
    Ok(Json(config.redacted_for_api_clone()))
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
    async fn get_config_returns_expected_fields() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::get("/api/config")
                    .header("Cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        // Must include runtime flags.
        assert!(json.get("jira_available").is_some());
        assert!(json.get("config_writable").is_some());
        assert!(json.get("ticketing_system").is_some());
        assert_eq!(json["jira_available"], false);
        assert_eq!(json["config_writable"], false);
        assert_eq!(json["ticketing_system"], "none");
        // repo_exists should be present (value depends on the test environment)
        assert!(json.get("repo_exists").is_some());
        assert!(json["repo_exists"].is_boolean());
    }

    #[tokio::test]
    async fn put_config_valid_patch_updates_value() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::put("/api/config")
                    .header("Content-Type", "application/json")
                    .header("Origin", "http://localhost:8080")
                    .header("Cookie", &cookie)
                    .body(Body::from(r#"{"general":{"max_concurrent_workflows":5}}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["general"]["max_concurrent_workflows"], 5);

        // Verify it's also updated in the in-memory config.
        let cfg = state.config.config.read().await;
        assert_eq!(cfg.general.max_concurrent_workflows, 5);
    }

    #[tokio::test]
    async fn put_config_unknown_field_returns_400() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::put("/api/config")
                    .header("Content-Type", "application/json")
                    .header("Origin", "http://localhost:8080")
                    .header("Cookie", &cookie)
                    .body(Body::from(r#"{"jira":{"site":"x"}}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        // deny_unknown_fields on RuntimeDashboardConfigPatch should reject "jira".
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn put_config_ticketing_system_jira_is_accepted_and_applied() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::put("/api/config")
                    .header("Content-Type", "application/json")
                    .header("Origin", "http://localhost:8080")
                    .header("Cookie", &cookie)
                    .body(Body::from(
                        r#"{"general":{"ticketing_system":"jira"}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["general"]["ticketing_system"], "jira");

        let cfg = state.config.config.read().await;
        assert_eq!(
            cfg.general.ticketing_system,
            takuto_core::config::TicketingSystem::Jira,
        );
    }

    #[tokio::test]
    async fn post_config_reload_without_config_file_returns_400() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        let app = build_router(state);
        // config_path points to a non-existent temp file, so reload should fail.
        let resp = app
            .oneshot(
                Request::post("/api/config/reload")
                    .header("Origin", "http://localhost:8080")
                    .header("Cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
