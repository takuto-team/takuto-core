use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;

use maestro_core::config::{Config, RuntimeDashboardConfigPatch};

use crate::state::AppState;

pub async fn get_config(State(state): State<AppState>) -> Json<Config> {
    let config = state.config.read().await;
    Json(config.redacted_for_api_clone())
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
