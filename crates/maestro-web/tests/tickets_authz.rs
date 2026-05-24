// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

// Copyright (C) 2026 Alexandre Obellianne
//
// Integration tests for AC-2 — IDOR fixes on /api/tickets/{key}/*.
//
// For each of the three ticket-action endpoints:
//   - POST /api/tickets/{key}/improve
//   - POST /api/tickets/{key}/prompt
//   - POST /api/tickets/{key}/update-description
//
// we assert:
//   - G/W/T 2.x: bob calling alice's ticket → 404 Not Found (existence is not
//     leaked).
//   - G/W/T 2.5: any user calling a non-existent ticket → 404 (same response
//     as the cross-user case).
//   - G/W/T 2.4: alice calling her own ticket → non-404 (handler executes).
//
// The improve / prompt handlers normally invoke a real AI session. We flip the
// dev-mock for this process via `MockGuard::on()` so they short-circuit to a
// scripted response without contacting Claude/Cursor.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use maestro_core::config::TicketingSystem;
use maestro_core::dev_mock::MockGuard;
use maestro_core::workflow::engine::Workflow;

use maestro_web::server::build_router;
use maestro_web::state::AppState;
use maestro_web::test_helpers::{TEST_ORIGIN, register_and_login, test_state_with_db};

/// Create a second user (`role = user`) via the admin API and log them in.
async fn create_and_login_regular_user(
    state: &AppState,
    admin_cookie: &str,
    username: &str,
    password: &str,
) -> String {
    let app = build_router(state.clone());
    let body = format!(r#"{{"username":"{username}","password":"{password}","role":"user"}}"#);
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
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "creating regular user should succeed"
    );

    let app = build_router(state.clone());
    let body = format!(r#"{{"username":"{username}","password":"{password}"}}"#);
    let login_resp = app
        .oneshot(
            Request::post("/api/auth/login")
                .header("Content-Type", "application/json")
                .header("Origin", TEST_ORIGIN)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login_resp.status(), StatusCode::NO_CONTENT);
    let set_cookie = login_resp
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    set_cookie.split(';').next().unwrap().trim().to_string()
}

/// Insert a workflow with the given user_id directly into the engine map.
///
/// Plan-10: the workflow's `workspace_name` must match a `repositories` row
/// that the owning user has added to their `user_repositories`, otherwise
/// `require_workflow_access` filters it out. Seed both alongside the workflow
/// so the post-plan-10 visibility gate matches the test's intent.
async fn seed_workflow(state: &AppState, key: &str, user_id: &str) {
    let workspace = "test-workspace".to_string();
    let mut wf = Workflow::new(
        key.to_string(),
        format!("Summary for {key}"),
        false,
        false,
        TicketingSystem::None,
        None,
        workspace.clone(),
    );
    wf.user_id = Some(user_id.to_string());
    let repository_id = {
        let db = state.auth.db.as_ref().expect("test state must have a DB").clone();
        let user_id_owned = user_id.to_string();
        let workspace_owned = workspace.clone();
        tokio::task::spawn_blocking(move || {
            let conn = db.conn().blocking_lock();
            let id = maestro_core::db::repositories::upsert(
                &conn,
                &workspace_owned,
                None,
                &format!("/workspaces/{workspace_owned}"),
                "main",
                None,
            )
            .expect("seed repositories row");
            // Only attempt the user_repositories association when the user_id
            // is real (FK to users.id). Tests that pass a synthetic user_id
            // (e.g. "fake-user-id" for the 401 path) skip the association —
            // the workflow exists in the engine map but `require_workflow_access`
            // is never reached because auth rejects the request first.
            let _ = maestro_core::db::repositories::add_for_user(&conn, &user_id_owned, &id);
            id
        })
        .await
        .expect("join")
    };
    wf.repository_id = Some(repository_id);
    state
        .engine
        .engine
        .workflows_arc()
        .write()
        .await
        .insert(key.to_string(), wf);
}

/// Look up `user_id` from the database by username — needed so the seeded
/// workflow's `user_id` matches the cookie-derived auth user_id.
async fn user_id_for(state: &AppState, username: &str) -> String {
    let db = state.auth.db.clone().expect("test state must have a DB");
    let username = username.to_string();
    tokio::task::spawn_blocking(move || {
        let conn = db.conn().blocking_lock();
        let user = maestro_core::db::users::get_user_by_username(&conn, &username)
            .expect("db query")
            .expect("user must exist");
        user.id
    })
    .await
    .unwrap()
}

const IMPROVE_BODY: &str = r#"{"description":"x","summary":"y"}"#;
const PROMPT_BODY: &str = r#"{"prompt":"hi","ticket_title":"y","ticket_description":"x"}"#;
const UPDATE_BODY: &str = r#"{"description":"x"}"#;

/// Endpoints to exercise. Listed as `(path-suffix, body, name-for-asserts)`.
const ENDPOINTS: &[(&str, &str, &str)] = &[
    ("improve", IMPROVE_BODY, "improve"),
    ("prompt", PROMPT_BODY, "prompt"),
    ("update-description", UPDATE_BODY, "update-description"),
];

fn post(state: &AppState, path: &str, cookie: &str, body: &'static str) -> Request<Body> {
    let _ = state; // marker
    Request::post(path)
        .header("Content-Type", "application/json")
        .header("Origin", TEST_ORIGIN)
        .header("Cookie", cookie)
        .body(Body::from(body))
        .unwrap()
}

#[tokio::test]
async fn bob_cannot_act_on_alices_ticket() {
    // Mock the agent so improve/prompt don't try to contact Claude/Cursor.
    let _guard = MockGuard::on();

    let state = test_state_with_db();
    let alice_cookie = register_and_login(&state).await; // first user = admin
    let bob_cookie =
        create_and_login_regular_user(&state, &alice_cookie, "bob", "secretpassword123!").await;

    let alice_id = user_id_for(&state, "admin").await;
    seed_workflow(&state, "ALICE-1", &alice_id).await;

    for (suffix, body, name) in ENDPOINTS {
        let app = build_router(state.clone());
        let path = format!("/api/tickets/ALICE-1/{suffix}");
        let req = post(&state, &path, &bob_cookie, body);
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "bob on alice's ticket via {name}: expected 404, got {}",
            resp.status()
        );
    }
}

#[tokio::test]
async fn non_existent_ticket_returns_404_for_any_user() {
    let _guard = MockGuard::on();

    let state = test_state_with_db();
    let alice_cookie = register_and_login(&state).await;
    // No workflow inserted — every action against NOPE-999 should 404.

    for (suffix, body, name) in ENDPOINTS {
        let app = build_router(state.clone());
        let path = format!("/api/tickets/NOPE-999/{suffix}");
        let req = post(&state, &path, &alice_cookie, body);
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "alice on non-existent ticket via {name}: expected 404, got {}",
            resp.status()
        );
    }
}

#[tokio::test]
async fn alice_can_act_on_her_own_ticket() {
    let _guard = MockGuard::on();

    let state = test_state_with_db();
    let alice_cookie = register_and_login(&state).await;
    let alice_id = user_id_for(&state, "admin").await;
    seed_workflow(&state, "ALICE-2", &alice_id).await;

    for (suffix, body, name) in ENDPOINTS {
        let app = build_router(state.clone());
        let path = format!("/api/tickets/ALICE-2/{suffix}");
        let req = post(&state, &path, &alice_cookie, body);
        let resp = app.oneshot(req).await.unwrap();
        assert_ne!(
            resp.status(),
            StatusCode::NOT_FOUND,
            "alice on her own ticket via {name}: must NOT be 404 (got {})",
            resp.status()
        );
        assert_ne!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "alice on her own ticket via {name}: must NOT be 401 (got {})",
            resp.status()
        );
        assert_ne!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "alice on her own ticket via {name}: must NOT be 403 (got {})",
            resp.status()
        );
    }
}

#[tokio::test]
async fn unauthenticated_request_is_401() {
    let state = test_state_with_db();
    // Seed a workflow under a fake user id so the workflow exists, but call
    // without a cookie to verify the middleware still rejects.
    seed_workflow(&state, "ANY-1", "fake-user-id").await;

    for (suffix, body, name) in ENDPOINTS {
        let app = build_router(state.clone());
        let path = format!("/api/tickets/ANY-1/{suffix}");
        let resp = app
            .oneshot(
                Request::post(&path)
                    .header("Content-Type", "application/json")
                    // Send a valid Origin so the CSRF middleware passes and
                    // the auth middleware is the one that rejects.
                    .header("Origin", TEST_ORIGIN)
                    .body(Body::from(*body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "unauthenticated {name}: expected 401, got {}",
            resp.status()
        );
    }
}
