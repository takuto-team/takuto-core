// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Integration coverage for the `/api/me/flows` CRUD surface.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use maestro_core::db::user_work_item_flows::{UserFlow, UserFlowStep};
use maestro_web::server::build_router;
use maestro_web::state::AppState;
use maestro_web::test_helpers::{TEST_ORIGIN, register_and_login, test_state_with_db};
use serde_json::{Value, json};
use tower::ServiceExt;

fn flow(name: &str, deps: &[&str]) -> UserFlow {
    UserFlow {
        name: name.to_string(),
        depends_on: deps.iter().map(|s| s.to_string()).collect(),
        steps: vec![UserFlowStep {
            name: "step".to_string(),
            prompt: "do it".to_string(),
            skills: Vec::new(),
        }],
    }
}

fn sample_defaults() -> Vec<UserFlow> {
    vec![
        flow("Implement Ticket", &[]),
        flow("Review", &["Implement Ticket"]),
    ]
}

/// State seeded with non-empty flow defaults plus an authenticated cookie.
async fn state_with_defaults() -> (AppState, String) {
    let mut state = test_state_with_db();
    state.config_mut().work_item_flow_defaults = Arc::new(sample_defaults());
    let cookie = register_and_login(&state).await;
    (state, cookie)
}

async fn body_json(resp: axum::response::Response) -> Value {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn get_empty_seeds_and_returns_defaults() {
    let (state, cookie) = state_with_defaults().await;

    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::get("/api/me/flows")
                .header("Cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = body_json(resp).await;
    assert!(
        body["workspace"].as_str().is_some_and(|w| !w.is_empty()),
        "workspace must be present: {body}"
    );
    let names: Vec<&str> = body["flows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["Implement Ticket", "Review"]);
}

#[tokio::test]
async fn put_valid_round_trips() {
    let (state, cookie) = state_with_defaults().await;

    let payload = json!({ "flows": [flow("Only Flow", &[])] });
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::put("/api/me/flows")
                .header("Content-Type", "application/json")
                .header("Origin", TEST_ORIGIN)
                .header("Cookie", &cookie)
                .body(Body::from(serde_json::to_vec(&payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // GET returns the persisted single flow, not the defaults.
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::get("/api/me/flows")
                .header("Cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_json(resp).await;
    let flows = body["flows"].as_array().unwrap();
    assert_eq!(flows.len(), 1);
    assert_eq!(flows[0]["name"], "Only Flow");
}

#[tokio::test]
async fn put_with_cycle_returns_400() {
    let (state, cookie) = state_with_defaults().await;

    let payload = json!({ "flows": [flow("A", &["B"]), flow("B", &["A"])] });
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::put("/api/me/flows")
                .header("Content-Type", "application/json")
                .header("Origin", TEST_ORIGIN)
                .header("Cookie", &cookie)
                .body(Body::from(serde_json::to_vec(&payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp).await;
    assert_eq!(body["kind"], "dependency_cycle", "structured body: {body}");
}

#[tokio::test]
async fn put_with_21_flows_returns_400() {
    let (state, cookie) = state_with_defaults().await;

    let flows: Vec<UserFlow> = (0..21).map(|i| flow(&format!("Flow {i}"), &[])).collect();
    let payload = json!({ "flows": flows });
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::put("/api/me/flows")
                .header("Content-Type", "application/json")
                .header("Origin", TEST_ORIGIN)
                .header("Cookie", &cookie)
                .body(Body::from(serde_json::to_vec(&payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp).await;
    assert_eq!(body["kind"], "too_many_flows", "structured body: {body}");
}

#[tokio::test]
async fn reseed_overwrites_with_defaults() {
    let (state, cookie) = state_with_defaults().await;

    // Empty the list first.
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::put("/api/me/flows")
                .header("Content-Type", "application/json")
                .header("Origin", TEST_ORIGIN)
                .header("Cookie", &cookie)
                .body(Body::from(r#"{"flows":[]}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Reseed restores the defaults.
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::post("/api/me/flows/reseed")
                .header("Origin", TEST_ORIGIN)
                .header("Cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp).await;
    let names: Vec<&str> = body["flows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["Implement Ticket", "Review"]);
}
