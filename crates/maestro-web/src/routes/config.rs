use std::sync::atomic::Ordering;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::Serialize;

use maestro_core::config::{Config, RuntimeDashboardConfigPatch};

use crate::state::AppState;

/// Wraps the redacted config with extra runtime flags that are not in `config.toml`.
#[derive(Serialize)]
pub struct ConfigResponse {
    #[serde(flatten)]
    pub config: Config,
    /// `true` when acli (Jira) is authenticated.
    pub jira_available: bool,
}

pub async fn get_config(State(state): State<AppState>) -> Json<ConfigResponse> {
    let config = state.config.read().await;
    Json(ConfigResponse {
        config: config.redacted_for_api_clone(),
        jira_available: state.jira_available.load(Ordering::Relaxed),
    })
}

pub async fn update_config(
    State(state): State<AppState>,
    Json(patch): Json<RuntimeDashboardConfigPatch>,
) -> Result<Json<Config>, (StatusCode, String)> {
    let mut config = state.config.write().await;
    config
        .apply_runtime_dashboard_patch(patch)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    Ok(Json(config.redacted_for_api_clone()))
}
