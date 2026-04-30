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

    #[tokio::test]
    async fn get_polling_returns_paused_false_initially() {
        let state = test_state();
        let app = build_router(state);
        let resp = app
            .oneshot(Request::get("/api/polling").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["paused"], false);
    }

    #[tokio::test]
    async fn pause_then_get_returns_paused_true() {
        let state = test_state();

        // Pause polling.
        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::post("/api/polling/pause")
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
            .oneshot(Request::get("/api/polling").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["paused"], true);
    }

    #[tokio::test]
    async fn resume_after_pause_returns_paused_false() {
        let state = test_state();

        // Pause first.
        let app = build_router(state.clone());
        app.oneshot(
            Request::post("/api/polling/pause")
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
            .oneshot(Request::get("/api/polling").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["paused"], false);
    }
}
