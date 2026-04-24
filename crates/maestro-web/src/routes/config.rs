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
}

pub async fn get_version() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "version": maestro_core::VERSION.trim()
    }))
}

pub async fn get_config(State(state): State<AppState>) -> Json<ConfigResponse> {
    let config = state.config.read().await;
    Json(ConfigResponse {
        github_app_configured: config.github.is_configured(),
        config: config.redacted_for_api_clone(),
        jira_available: state.jira_available.load(Ordering::Relaxed),
        ticketing_system: state.ticketing_system.to_string(),
        preflight_error: state.preflight_error.clone(),
        config_writable: state.config_writer.is_some(),
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
