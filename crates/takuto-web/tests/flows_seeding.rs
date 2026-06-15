// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Seeding of per-user work-item flows at user-creation time.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use takuto_core::db::user_work_item_flows::{self, UserFlow, UserFlowStep};
use takuto_web::server::build_router;
use takuto_web::test_helpers::{TEST_ORIGIN, test_state_with_db};
use tower::ServiceExt;

fn sample_defaults() -> Vec<UserFlow> {
    vec![UserFlow {
        name: "Implement Ticket".to_string(),
        depends_on: Vec::new(),
        steps: vec![UserFlowStep {
            name: "Code it".to_string(),
            prompt: "Implement the feature".to_string(),
            skills: Vec::new(),
        }],
    }]
}

/// First-user registration seeds the new admin's flows for the active
/// workspace from the configured TOML defaults.
#[tokio::test]
async fn register_seeds_default_flows_for_first_user() {
    let mut state = test_state_with_db();
    state.config_mut().work_item_flow_defaults = Arc::new(sample_defaults());

    let workspace = state.config().active_workspace_name().await;
    let db = state.auth().db.clone().expect("db present");
    let adapter = db.adapter();

    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::post("/api/auth/register")
                .header("Content-Type", "application/json")
                .header("Origin", TEST_ORIGIN)
                .body(Body::from(
                    r#"{"username":"admin","password":"testpassword1234"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let user = takuto_core::db::users::get_user_by_username(adapter, "admin")
        .await
        .unwrap()
        .expect("admin user exists after register");

    let flows = user_work_item_flows::get(adapter, &user.id, &workspace)
        .await
        .unwrap()
        .expect("a flow row must have been seeded");
    assert_eq!(flows, sample_defaults(), "seeded flows match the defaults");
}

/// Admin-created users are seeded at creation time for the active workspace.
#[tokio::test]
async fn admin_create_user_seeds_default_flows() {
    let mut state = test_state_with_db();
    state.config_mut().work_item_flow_defaults = Arc::new(sample_defaults());

    let workspace = state.config().active_workspace_name().await;
    let db = state.auth().db.clone().expect("db present");
    let adapter = db.adapter();

    // First user (admin) to obtain a session cookie.
    let cookie = takuto_web::test_helpers::register_and_login(&state).await;

    // Admin creates a second user.
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::post("/api/users")
                .header("Content-Type", "application/json")
                .header("Origin", TEST_ORIGIN)
                .header("Cookie", &cookie)
                .body(Body::from(r#"{"username":"bob","role":"user"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let bob = takuto_core::db::users::get_user_by_username(adapter, "bob")
        .await
        .unwrap()
        .expect("bob exists after create");

    let flows = user_work_item_flows::get(adapter, &bob.id, &workspace)
        .await
        .unwrap()
        .expect("bob's flow row must have been seeded at creation");
    assert_eq!(flows, sample_defaults());
}
