// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Runtime polling pause/resume.
//!
//! These endpoints read and flip the live `EngineState.polling_paused` flag the
//! Jira/GitHub pollers consult each cycle. `GET /api/polling` reports the real
//! state; `POST /api/polling/{pause,resume}` toggle it (admin-gated). The flip
//! is runtime-only — it is not persisted to `config.toml`; the persisted
//! enable/disable lives in `[general] auto_polling` (Configuration → Item
//! Polling), which also flips this same flag.

use std::sync::atomic::Ordering;

use axum::extract::{Extension, State};
use axum::http::StatusCode;
use serde::Serialize;

use crate::auth::AuthenticatedUser;
use crate::routes::admin::require_admin_for;
use crate::state::{AuthState, EngineState};

/// Polling-status payload. `paused` mirrors `EngineState.polling_paused`;
/// `reason` is a short human string the UI may surface verbatim.
#[derive(Serialize)]
pub struct PollingStatus {
    pub paused: bool,
    pub reason: &'static str,
}

fn status(paused: bool) -> PollingStatus {
    PollingStatus {
        paused,
        reason: if paused {
            "polling paused"
        } else {
            "polling active"
        },
    }
}

pub async fn get_polling_status(
    State(engine_state): State<EngineState>,
) -> axum::Json<PollingStatus> {
    axum::Json(status(engine_state.polling_paused.load(Ordering::Relaxed)))
}

pub async fn pause_polling(
    State(auth_state): State<AuthState>,
    State(engine_state): State<EngineState>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<axum::Json<PollingStatus>, StatusCode> {
    require_admin_for(&auth_state, &auth).await?;
    engine_state.polling_paused.store(true, Ordering::Relaxed);
    Ok(axum::Json(status(true)))
}

pub async fn resume_polling(
    State(auth_state): State<AuthState>,
    State(engine_state): State<EngineState>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<axum::Json<PollingStatus>, StatusCode> {
    require_admin_for(&auth_state, &auth).await?;
    engine_state.polling_paused.store(false, Ordering::Relaxed);
    Ok(axum::Json(status(false)))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::server::build_router;
    use crate::test_helpers::{TEST_ORIGIN, register_and_login, test_state_with_db};

    #[tokio::test]
    async fn get_polling_reports_live_flag() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;
        // Harness starts un-paused.
        assert!(!state.engine.polling_paused.load(Ordering::Relaxed));

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::get("/api/polling")
                    .header("Cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["paused"], false);
    }

    #[tokio::test]
    async fn pause_then_resume_flips_the_live_flag() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::post("/api/polling/pause")
                    .header("Origin", TEST_ORIGIN)
                    .header("Cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["paused"], true);
        assert!(state.engine.polling_paused.load(Ordering::Relaxed));

        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::post("/api/polling/resume")
                    .header("Origin", TEST_ORIGIN)
                    .header("Cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["paused"], false);
        assert!(!state.engine.polling_paused.load(Ordering::Relaxed));
    }
}
