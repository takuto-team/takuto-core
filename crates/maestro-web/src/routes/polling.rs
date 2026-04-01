use std::sync::atomic::Ordering;

use axum::extract::State;
use serde::Serialize;

use crate::state::AppState;

#[derive(Serialize)]
pub struct PollingStatus {
    pub paused: bool,
}

pub async fn get_polling_status(State(state): State<AppState>) -> axum::Json<PollingStatus> {
    axum::Json(PollingStatus {
        paused: state.polling_paused.load(Ordering::Relaxed),
    })
}

pub async fn pause_polling(State(state): State<AppState>) -> axum::Json<PollingStatus> {
    state.polling_paused.store(true, Ordering::Relaxed);
    axum::Json(PollingStatus { paused: true })
}

pub async fn resume_polling(State(state): State<AppState>) -> axum::Json<PollingStatus> {
    state.polling_paused.store(false, Ordering::Relaxed);
    axum::Json(PollingStatus { paused: false })
}
