// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

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

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::server::build_router;
    use crate::test_helpers::{register_and_login, test_state_with_db};

    #[tokio::test]
    async fn get_polling_returns_paused_false_initially() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

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
    async fn pause_then_get_returns_paused_true() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        // Pause polling.
        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::post("/api/polling/pause")
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

        // Verify via GET.
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
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["paused"], true);
    }

    #[tokio::test]
    async fn resume_after_pause_returns_paused_false() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        // Pause first.
        let app = build_router(state.clone());
        app.oneshot(
            Request::post("/api/polling/pause")
                .header("Cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

        // Resume.
        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::post("/api/polling/resume")
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

        // Verify via GET.
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
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["paused"], false);
    }
}
