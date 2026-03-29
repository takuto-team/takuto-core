use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;

use maestro_core::config::Config;

use crate::state::AppState;

pub async fn get_config(State(state): State<AppState>) -> Json<Config> {
    let config = state.config.read().await;
    Json(config.clone())
}

pub async fn update_config(
    State(state): State<AppState>,
    Json(new_config): Json<Config>,
) -> Result<Json<Config>, (StatusCode, String)> {
    if let Err(e) = new_config.validate() {
        return Err((StatusCode::BAD_REQUEST, e.to_string()));
    }

    let mut config = state.config.write().await;
    *config = new_config.clone();
    Ok(Json(new_config))
}
