// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::sync::atomic::Ordering;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::Serialize;

use maestro_core::config::{Config, RuntimeDashboardConfigPatch};
use maestro_core::config_watcher::reload_config_from_disk;

use crate::state::AppState;

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
}

/// Parse the `origin` remote URL from `.git/config` and convert it to an
/// `https://github.com/…` HTML URL (handles both HTTPS and SSH remotes).
fn read_git_remote_url(repo_path: &std::path::Path) -> Option<String> {
    let git_config = std::fs::read_to_string(repo_path.join(".git/config")).ok()?;
    // Find the `[remote "origin"]` section and extract the `url =` line.
    let mut in_origin = false;
    for line in git_config.lines() {
        let trimmed = line.trim();
        if trimmed == r#"[remote "origin"]"# {
            in_origin = true;
            continue;
        }
        if in_origin {
            if trimmed.starts_with('[') {
                break; // moved to next section
            }
            if let Some(rest) = trimmed.strip_prefix("url =") {
                let url = rest.trim();
                return Some(normalize_github_url(url));
            }
        }
    }
    None
}

/// Convert any GitHub remote URL form to an `https://github.com/owner/repo` HTML URL.
fn normalize_github_url(url: &str) -> String {
    // SSH form: git@github.com:owner/repo.git
    if let Some(path) = url.strip_prefix("git@github.com:") {
        let path = path.trim_end_matches(".git");
        return format!("https://github.com/{path}");
    }
    // HTTPS form: https://github.com/owner/repo.git
    let url = url.trim_end_matches(".git");
    url.to_string()
}

pub async fn get_version() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "version": maestro_core::VERSION.trim()
    }))
}

pub async fn get_config(State(state): State<AppState>) -> Json<ConfigResponse> {
    let config = state.config.read().await;
    // `repo_exists` scans /workspaces/ for any directory that contains a .git
    // entry, so it reflects the actual filesystem state rather than a potentially
    // stale config value.
    // Scan /workspaces/ for the first directory containing a .git entry.
    let repo_dir = std::fs::read_dir("/workspaces")
        .ok()
        .and_then(|entries| {
            entries
                .filter_map(|e| e.ok())
                .find(|e| e.path().join(".git").exists())
        });
    let repo_exists = repo_dir.is_some();
    let repo_name = repo_dir
        .as_ref()
        .map(|e| e.file_name().to_string_lossy().into_owned());
    let repo_html_url = repo_dir.as_ref().and_then(|e| {
        read_git_remote_url(&e.path())
    });
    Json(ConfigResponse {
        github_app_configured: config.github.is_configured(),
        config: config.redacted_for_api_clone(),
        jira_available: state.jira_available.load(Ordering::Relaxed),
        ticketing_system: state.ticketing_system.to_string(),
        preflight_error: state.preflight_error.clone(),
        config_writable: state.config_writer.is_some(),
        repo_exists,
        repo_name,
        repo_html_url,
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
    State(state): State<AppState>,
    Json(patch): Json<RuntimeDashboardConfigPatch>,
) -> Result<Json<UpdateConfigResponse>, (StatusCode, String)> {
    // Apply patch under write lock, then clone and release.
    let config_snapshot = {
        let mut config = state.config.write().await;
        config
            .apply_runtime_dashboard_patch(patch)
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        config.clone()
    }; // write lock dropped here

    // Persist to disk OUTSIDE the lock — blocking I/O no longer holds the
    // write guard, so concurrent readers are not stalled.
    let (persisted, persist_warning) = if let Some(ref writer) = state.config_writer {
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
    State(state): State<AppState>,
) -> Result<Json<Config>, (StatusCode, String)> {
    reload_config_from_disk(&state.config_path, &state.config)
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                format!("Config reload failed: {e}"),
            )
        })?;

    let config = state.config.read().await;
    Ok(Json(config.redacted_for_api_clone()))
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

    fn test_state() -> AppState {
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
        }
    }

    fn test_state_with_password() -> AppState {
        let mut cfg = Config::default();
        cfg.web.dashboard_username = "admin".to_string();
        cfg.web.dashboard_password = "supersecret".to_string();
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
        }
    }

    #[tokio::test]
    async fn get_config_returns_expected_fields() {
        let state = test_state();
        let app = build_router(state);
        let resp = app
            .oneshot(Request::get("/api/config").body(Body::empty()).unwrap())
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
    async fn get_config_does_not_return_dashboard_password() {
        let state = test_state_with_password();
        // Auth is enabled, so we need a valid session cookie.
        let app = build_router(state.clone());
        let login_resp = app
            .oneshot(
                Request::post("/api/auth/login")
                    .header("Content-Type", "application/json")
                    .body(Body::from(
                        r#"{"username":"admin","password":"supersecret"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let cookie = login_resp
            .headers()
            .get("set-cookie")
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .trim()
            .to_string();

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
        let password = json
            .pointer("/web/dashboard_password")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert!(
            password.is_empty(),
            "dashboard_password must be redacted, got: {password}"
        );
    }

    #[tokio::test]
    async fn put_config_valid_patch_updates_value() {
        let state = test_state();
        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::put("/api/config")
                    .header("Content-Type", "application/json")
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
        let cfg = state.config.read().await;
        assert_eq!(cfg.general.max_concurrent_workflows, 5);
    }

    #[tokio::test]
    async fn put_config_unknown_field_returns_400() {
        let state = test_state();
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::put("/api/config")
                    .header("Content-Type", "application/json")
                    .body(Body::from(r#"{"jira":{"site":"x"}}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        // deny_unknown_fields on RuntimeDashboardConfigPatch should reject "jira".
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn put_config_empty_password_preserves_existing() {
        let state = test_state_with_password();
        // Login first to get a session cookie.
        let app = build_router(state.clone());
        let login_resp = app
            .oneshot(
                Request::post("/api/auth/login")
                    .header("Content-Type", "application/json")
                    .body(Body::from(
                        r#"{"username":"admin","password":"supersecret"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let cookie = login_resp
            .headers()
            .get("set-cookie")
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .trim()
            .to_string();

        // Send a PUT that updates the username but sends empty password.
        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::put("/api/config")
                    .header("Content-Type", "application/json")
                    .header("Cookie", &cookie)
                    .body(Body::from(
                        r#"{"web":{"dashboard_username":"admin","dashboard_password":""}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Verify the password was preserved (not cleared).
        let cfg = state.config.read().await;
        assert_eq!(cfg.web.dashboard_password, "supersecret");
    }

    #[tokio::test]
    async fn post_config_reload_without_config_file_returns_400() {
        let state = test_state();
        let app = build_router(state);
        // config_path points to a non-existent temp file, so reload should fail.
        let resp = app
            .oneshot(
                Request::post("/api/config/reload")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
