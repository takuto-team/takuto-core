// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Integration coverage for the `/api/me/flows` CRUD surface.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use maestro_core::config::SkillRef;
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

/// Build a flow with explicit steps so individual validation rules can be
/// violated one at a time.
fn flow_with_steps(name: &str, deps: &[&str], steps: Vec<UserFlowStep>) -> UserFlow {
    UserFlow {
        name: name.to_string(),
        depends_on: deps.iter().map(|s| s.to_string()).collect(),
        steps,
    }
}

fn step(name: &str, prompt: &str) -> UserFlowStep {
    UserFlowStep {
        name: name.to_string(),
        prompt: prompt.to_string(),
        skills: Vec::new(),
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

async fn body_text(resp: axum::response::Response) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8_lossy(&bytes).to_string()
}

/// `PUT /api/me/flows` with a flow list, returning the raw response.
async fn put_flows(state: &AppState, cookie: &str, flows: &[UserFlow]) -> axum::response::Response {
    let payload = json!({ "flows": flows });
    let app = build_router(state.clone());
    app.oneshot(
        Request::put("/api/me/flows")
            .header("Content-Type", "application/json")
            .header("Origin", TEST_ORIGIN)
            .header("Cookie", cookie)
            .body(Body::from(serde_json::to_vec(&payload).unwrap()))
            .unwrap(),
    )
    .await
    .unwrap()
}

/// `GET /api/me/flows`, returning the ordered flow names.
async fn get_flow_names(state: &AppState, cookie: &str) -> Vec<String> {
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::get("/api/me/flows")
                .header("Cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    body_json(resp)
        .await
        .get("flows")
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap().to_string())
        .collect()
}

/// Point the active workspace at a fresh path so `active_workspace_name`
/// resolves to a distinct, never-seeded workspace.
async fn switch_workspace(state: &AppState, repo_path: &str) {
    state.config().config.write().await.git.repo_path = repo_path.to_string();
}

/// Assert a PUT is rejected with 400 and the given structured `kind`.
async fn assert_put_rejected(state: &AppState, cookie: &str, flows: &[UserFlow], kind: &str) {
    let resp = put_flows(state, cookie, flows).await;
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "expected 400 for kind {kind}"
    );
    let body = body_json(resp).await;
    assert_eq!(body["kind"], kind, "structured body: {body}");
    assert!(
        body["error"].as_str().is_some_and(|e| !e.is_empty()),
        "error message must be present: {body}"
    );
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

// ── empty-list round-trip ──────────────────────────────────────────────────

#[tokio::test]
async fn put_empty_list_persists_and_does_not_reseed() {
    let (state, cookie) = state_with_defaults().await;

    let resp = put_flows(&state, &cookie, &[]).await;
    assert_eq!(resp.status(), StatusCode::OK);

    // The empty list is the user's deliberate state — GET returns it as-is and
    // never re-seeds the defaults back in.
    assert!(
        get_flow_names(&state, &cookie).await.is_empty(),
        "an emptied list must stay empty across GET"
    );
}

// ── validation 400 matrix (one per FlowValidationError::kind) ────────────────

#[tokio::test]
async fn put_duplicate_name_returns_400() {
    let (state, cookie) = state_with_defaults().await;
    let flows = vec![flow("Build", &[]), flow("Build", &[])];
    assert_put_rejected(&state, &cookie, &flows, "duplicate_flow_name").await;
}

#[tokio::test]
async fn put_colliding_slug_returns_400() {
    let (state, cookie) = state_with_defaults().await;
    // Two distinct names that kebab-case to the same slug.
    let flows = vec![flow("Implement Ticket", &[]), flow("implement-ticket", &[])];
    assert_put_rejected(&state, &cookie, &flows, "duplicate_slug").await;
}

#[tokio::test]
async fn put_empty_name_returns_400() {
    let (state, cookie) = state_with_defaults().await;
    let flows = vec![flow("   ", &[])];
    assert_put_rejected(&state, &cookie, &flows, "empty_flow_name").await;
}

#[tokio::test]
async fn put_name_with_no_slug_chars_returns_400() {
    let (state, cookie) = state_with_defaults().await;
    let flows = vec![flow("---", &[])];
    assert_put_rejected(&state, &cookie, &flows, "empty_slug").await;
}

#[tokio::test]
async fn put_no_steps_returns_400() {
    let (state, cookie) = state_with_defaults().await;
    let flows = vec![flow_with_steps("Build", &[], vec![])];
    assert_put_rejected(&state, &cookie, &flows, "no_steps").await;
}

#[tokio::test]
async fn put_empty_step_name_returns_400() {
    let (state, cookie) = state_with_defaults().await;
    let flows = vec![flow_with_steps("Build", &[], vec![step("  ", "do it")])];
    assert_put_rejected(&state, &cookie, &flows, "empty_step_name").await;
}

#[tokio::test]
async fn put_empty_step_prompt_returns_400() {
    let (state, cookie) = state_with_defaults().await;
    let flows = vec![flow_with_steps("Build", &[], vec![step("compile", "   ")])];
    assert_put_rejected(&state, &cookie, &flows, "empty_step_prompt").await;
}

#[tokio::test]
async fn put_empty_skill_name_returns_400() {
    let (state, cookie) = state_with_defaults().await;
    let flows = vec![flow_with_steps(
        "Build",
        &[],
        vec![UserFlowStep {
            name: "compile".to_string(),
            prompt: "do it".to_string(),
            skills: vec![SkillRef {
                name: "  ".to_string(),
                args: vec![],
            }],
        }],
    )];
    assert_put_rejected(&state, &cookie, &flows, "empty_skill_name").await;
}

#[tokio::test]
async fn put_unknown_dependency_returns_400() {
    let (state, cookie) = state_with_defaults().await;
    let flows = vec![flow("Test", &["Ghost"])];
    assert_put_rejected(&state, &cookie, &flows, "unknown_dependency").await;
}

/// A NUL byte is caught by the DAO guard (after structural validation passes),
/// so it returns 400 — but as a plain-text message, not the `{error, kind}`
/// shape the structural validator emits.
#[tokio::test]
async fn put_nul_byte_returns_400() {
    let (state, cookie) = state_with_defaults().await;
    let flows = vec![flow("Bad\0Name", &[])];
    let resp = put_flows(&state, &cookie, &flows).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_text(resp).await;
    assert!(
        body.to_lowercase().contains("nul"),
        "body should name the NUL byte: {body}"
    );

    // Nothing was persisted by the rejected write.
    assert_eq!(
        get_flow_names(&state, &cookie).await,
        vec!["Implement Ticket", "Review"],
        "a rejected PUT must not mutate the stored list"
    );
}

// ── isolation: per workspace and per user ────────────────────────────────────

#[tokio::test]
async fn flows_are_isolated_per_workspace() {
    let (state, cookie) = state_with_defaults().await;

    // Workspace "alpha": save a bespoke list.
    switch_workspace(&state, "/tmp/maestro-ws-alpha").await;
    let resp = put_flows(&state, &cookie, &[flow("Alpha Only", &[])]).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(get_flow_names(&state, &cookie).await, vec!["Alpha Only"]);

    // Switch to a never-visited workspace "bravo": GET lazy-seeds the TOML
    // defaults there — it must NOT inherit alpha's bespoke list.
    switch_workspace(&state, "/tmp/maestro-ws-bravo").await;
    assert_eq!(
        get_flow_names(&state, &cookie).await,
        vec!["Implement Ticket", "Review"],
        "a new workspace lazy-seeds defaults, independent of other workspaces"
    );
    // Customise bravo differently.
    let resp = put_flows(&state, &cookie, &[flow("Bravo Only", &[])]).await;
    assert_eq!(resp.status(), StatusCode::OK);

    // Back to alpha: its list is untouched by anything done in bravo.
    switch_workspace(&state, "/tmp/maestro-ws-alpha").await;
    assert_eq!(get_flow_names(&state, &cookie).await, vec!["Alpha Only"]);
}

/// Create a second `role = user` account and return its session cookie.
async fn create_and_login_user(state: &AppState, admin_cookie: &str, username: &str) -> String {
    let app = build_router(state.clone());
    let body =
        format!(r#"{{"username":"{username}","password":"secretpassword123!@#","role":"user"}}"#);
    let resp = app
        .oneshot(
            Request::post("/api/users")
                .header("Content-Type", "application/json")
                .header("Origin", TEST_ORIGIN)
                .header("Cookie", admin_cookie)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED, "create user");

    let app = build_router(state.clone());
    let body = format!(r#"{{"username":"{username}","password":"secretpassword123!@#"}}"#);
    let login = app
        .oneshot(
            Request::post("/api/auth/login")
                .header("Content-Type", "application/json")
                .header("Origin", TEST_ORIGIN)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::NO_CONTENT, "login");
    let set_cookie = login
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    set_cookie.split(';').next().unwrap().trim().to_string()
}

#[tokio::test]
async fn flows_are_isolated_per_user_in_the_same_workspace() {
    let (state, admin_cookie) = state_with_defaults().await;
    let bob_cookie = create_and_login_user(&state, &admin_cookie, "bob").await;

    // The admin customises their own list in the active workspace.
    let resp = put_flows(&state, &admin_cookie, &[flow("Admin Only", &[])]).await;
    assert_eq!(resp.status(), StatusCode::OK);

    // The admin's change does not touch bob, who still has the seeded defaults.
    assert_eq!(
        get_flow_names(&state, &admin_cookie).await,
        vec!["Admin Only"]
    );
    assert_eq!(
        get_flow_names(&state, &bob_cookie).await,
        vec!["Implement Ticket", "Review"],
        "another user's list is unaffected"
    );
}

/// AC-12 round-trip: a valid list survives being read back through a fresh
/// router (a new request cycle against the same backing database).
#[tokio::test]
async fn valid_list_round_trips_across_requests() {
    let (state, cookie) = state_with_defaults().await;
    let flows = vec![
        flow_with_steps("Build", &[], vec![step("compile", "cargo build")]),
        flow_with_steps("Test", &["Build"], vec![step("unit", "cargo test")]),
    ];
    let resp = put_flows(&state, &cookie, &flows).await;
    assert_eq!(resp.status(), StatusCode::OK);

    assert_eq!(get_flow_names(&state, &cookie).await, vec!["Build", "Test"]);
}
