// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

// Copyright (C) 2026 Alexandre Obellianne
//
// Integration tests for IDOR fixes on /api/tickets/{key}/*.
//
// For each of the three ticket-action endpoints:
//   - POST /api/tickets/{key}/improve
//   - POST /api/tickets/{key}/prompt
//   - POST /api/tickets/{key}/update-description
//
// we assert:
//   - bob calling alice's *existing* ticket → 404 Not Found (ownership is
//     enforced and existence is not leaked).
//   - a non-existent ticket: improve/prompt are allowed (the Add-to-Dashboard
//     preview flow has no work item yet and they operate only on the request
//     body), while update-description still 404s in None mode (no preview
//     target, so existence is not leaked).
//   - alice calling her own ticket → non-404 (handler executes).
//
// The improve / prompt handlers normally invoke a real AI session. We flip the
// dev-mock for this process via `MockGuard::on()` so they short-circuit to a
// scripted response without contacting Claude/Cursor.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use takuto_core::config::TicketingSystem;
use takuto_core::dev_mock::MockGuard;
use takuto_core::workflow::engine::Workflow;

use takuto_web::server::build_router;
use takuto_web::state::AppState;
use takuto_web::test_helpers::{TEST_ORIGIN, register_and_login, test_state_with_db};

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
/// The workflow's `workspace_name` must match a `repositories` row that the
/// owning user has added to their `user_repositories`, otherwise
/// `require_workflow_access` filters it out. Seed both alongside the workflow
/// so the visibility gate matches the test's intent.
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
        let db = state.auth().db.as_ref().expect("test state must have a DB");
        let adapter = db.adapter();
        let id = takuto_core::db::repositories::upsert(
            adapter,
            &workspace,
            None,
            &format!("/workspaces/{workspace}"),
            "main",
            None,
        )
        .await
        .expect("seed repositories row");
        // Only attempt the user_repositories association when the user_id
        // is real (FK to users.id). Tests that pass a synthetic user_id
        // (e.g. "fake-user-id" for the 401 path) skip the association —
        // the workflow exists in the engine map but `require_workflow_access`
        // is never reached because auth rejects the request first.
        let _ = takuto_core::db::repositories::add_for_user(adapter, user_id, &id).await;
        id
    };
    wf.repository_id = Some(repository_id);
    state
        .engine()
        .engine
        .workflows_arc()
        .write()
        .await
        .insert(key.to_string(), wf);
}

/// Look up `user_id` from the database by username — needed so the seeded
/// workflow's `user_id` matches the cookie-derived auth user_id.
async fn user_id_for(state: &AppState, username: &str) -> String {
    let db = state.auth().db.clone().expect("test state must have a DB");
    let user = takuto_core::db::users::get_user_by_username(db.adapter(), username)
        .await
        .expect("db query")
        .expect("user must exist");
    user.id
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
async fn non_existent_ticket_allows_preview_endpoints_but_update_404s() {
    let _guard = MockGuard::on();

    let state = test_state_with_db();
    let alice_cookie = register_and_login(&state).await;
    // No workflow inserted. improve/prompt are offered in the Add-to-Dashboard
    // preview flow (no work item yet) and operate only on the caller-supplied
    // body, so they must NOT 404 on an unknown key. update-description has no
    // preview target in None mode, so it still 404s (existence is not leaked).

    for (suffix, body, name) in ENDPOINTS {
        let app = build_router(state.clone());
        let path = format!("/api/tickets/NOPE-999/{suffix}");
        let req = post(&state, &path, &alice_cookie, body);
        let resp = app.oneshot(req).await.unwrap();
        if *name == "update-description" {
            assert_eq!(
                resp.status(),
                StatusCode::NOT_FOUND,
                "update-description on a non-existent ticket: expected 404, got {}",
                resp.status()
            );
        } else {
            assert_ne!(
                resp.status(),
                StatusCode::NOT_FOUND,
                "{name} on a not-yet-added ticket should be allowed (preview flow), got 404"
            );
        }
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
