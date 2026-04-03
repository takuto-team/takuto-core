use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;

use maestro_core::config::Config;

use crate::state::AppState;

pub async fn get_config(State(state): State<AppState>) -> Json<Config> {
    let config = state.config.read().await;
    Json(config.redacted_for_api_clone())
}

pub async fn update_config(
    State(state): State<AppState>,
    Json(mut new_config): Json<Config>,
) -> Result<Json<Config>, (StatusCode, String)> {
    {
        let existing = state.config.read().await;
        if !new_config.web.dashboard_username.trim().is_empty()
            && new_config.web.dashboard_password.is_empty()
            && !existing.web.dashboard_password.is_empty()
        {
            new_config.web.dashboard_password = existing.web.dashboard_password.clone();
        }
    }

    if let Err(e) = new_config.validate() {
        return Err((StatusCode::BAD_REQUEST, e.to_string()));
    }

    let mut config = state.config.write().await;
    *config = new_config.clone();
    Ok(Json(new_config.redacted_for_api_clone()))
}
