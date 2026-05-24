// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Plan-10 Step 6 — workflow visibility is scoped to the caller's
//! `user_repositories` associations. The list endpoint must:
//!
//!   1. Show workflows whose `repository_id` is in the caller's added set.
//!   2. Hide workflows on a repo the caller has dropped (or never added),
//!      even if the workflow is owned by the caller.
//!   3. Never leak workflows owned by a different user, even if the caller
//!      and the owner have both added the same repository.
//!   4. Defensive back-compat: a workflow with `repository_id = None` but
//!      `workspace_name` matching a repo the user has added IS visible.
//!
//! All four assertions are exercised here with a real DB and a real engine,
//! seeding workflows directly into the in-memory map (no driver spawn).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use maestro_core::config::TicketingSystem;
use maestro_core::db::repositories;
use maestro_core::workflow::engine::Workflow;

use maestro_web::server::build_router;
use maestro_web::state::AppState;
use maestro_web::test_helpers::{TEST_ORIGIN, register_and_login, test_state_with_db};

/// Register a non-admin user via the admin endpoint and log them in. Returns
/// the session cookie ready for use in a `Cookie` header.
async fn create_and_login_user(
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
    assert_eq!(resp.status(), StatusCode::CREATED, "create user");

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

async fn user_id_for(state: &AppState, username: &str) -> String {
    let db = state.auth().db.clone().expect("test state must have a DB");
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

/// Register a repository row and (optionally) associate it with a user.
async fn seed_repository(
    state: &AppState,
    name: &str,
    local_path: &str,
    associate_with: &[&str],
) -> String {
    let db = state.auth().db.clone().expect("test state must have a DB");
    let name_owned = name.to_string();
    let local_path_owned = local_path.to_string();
    let user_ids: Vec<String> = associate_with.iter().map(|s| s.to_string()).collect();
    tokio::task::spawn_blocking(move || {
        let conn = db.conn().blocking_lock();
        let id = repositories::upsert(&conn, &name_owned, None, &local_path_owned, "main", None)
            .expect("repository upsert");
        for uid in &user_ids {
            repositories::add_for_user(&conn, uid, &id).expect("add_for_user");
        }
        id
    })
    .await
    .unwrap()
}

async fn seed_workflow(
    state: &AppState,
    key: &str,
    user_id: &str,
    workspace_name: &str,
    repository_id: Option<String>,
) {
    let mut wf = Workflow::new(
        key.to_string(),
        format!("Summary for {key}"),
        false,
        false,
        TicketingSystem::None,
        None,
        workspace_name.to_string(),
    );
    wf.user_id = Some(user_id.to_string());
    wf.repository_id = repository_id;
    state
        .engine()
        .engine
        .workflows_arc()
        .write()
        .await
        .insert(key.to_string(), wf);
}

/// GET /api/workflows and return the parsed list of ticket_keys (so tests
/// don't need to lock the full WorkflowSummary shape).
async fn list_workflow_keys(state: &AppState, cookie: &str) -> Vec<String> {
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::get("/api/workflows")
                .header("Cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "GET /api/workflows");
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    v.as_array()
        .unwrap()
        .iter()
        .map(|item| item["ticket_key"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn workflow_visible_when_user_has_added_repo() {
    let state = test_state_with_db();
    let alice_cookie = register_and_login(&state).await; // first user → admin
    let alice_id = user_id_for(&state, "admin").await;

    // Register repo R and add it to Alice's dashboard.
    let repo_r = seed_repository(&state, "repo-r", "/tmp/repo-r", &[&alice_id]).await;
    seed_workflow(&state, "AAA-1", &alice_id, "repo-r", Some(repo_r.clone())).await;

    let keys = list_workflow_keys(&state, &alice_cookie).await;
    assert_eq!(keys, vec!["AAA-1".to_string()]);
}

#[tokio::test]
async fn workflow_hidden_when_user_does_not_have_repo() {
    let state = test_state_with_db();
    let alice_cookie = register_and_login(&state).await;
    let alice_id = user_id_for(&state, "admin").await;

    // Register repo R but do NOT associate it with Alice.
    let repo_r = seed_repository(&state, "repo-r", "/tmp/repo-r", &[]).await;
    // Workflow owned by Alice but on a repo she hasn't added.
    seed_workflow(&state, "AAA-1", &alice_id, "repo-r", Some(repo_r)).await;

    let keys = list_workflow_keys(&state, &alice_cookie).await;
    assert!(
        keys.is_empty(),
        "workflow on a repo the user has not added must be hidden, got {keys:?}"
    );
}

#[tokio::test]
async fn user_cannot_see_other_users_workflow_even_on_shared_repo() {
    let state = test_state_with_db();
    let alice_cookie = register_and_login(&state).await;
    let alice_id = user_id_for(&state, "admin").await;
    let bob_cookie =
        create_and_login_user(&state, &alice_cookie, "bob", "testpassword1234").await;
    let bob_id = user_id_for(&state, "bob").await;

    // Both users add the same repo.
    let repo_r = seed_repository(&state, "repo-r", "/tmp/repo-r", &[&alice_id, &bob_id]).await;
    // Bob's workflow, on the shared repo.
    seed_workflow(&state, "BOB-1", &bob_id, "repo-r", Some(repo_r)).await;

    let alice_view = list_workflow_keys(&state, &alice_cookie).await;
    assert!(
        alice_view.is_empty(),
        "Alice must not see Bob's workflow even when they share a repo, got {alice_view:?}"
    );
    let bob_view = list_workflow_keys(&state, &bob_cookie).await;
    assert_eq!(bob_view, vec!["BOB-1".to_string()]);
}

#[tokio::test]
async fn defensive_back_compat_workspace_name_match() {
    // A workflow with `repository_id = None` but a `workspace_name` matching a
    // repo the user has added must still be visible. This covers restored
    // snapshots that have not yet been back-filled by Dev A's reconciliation.
    let state = test_state_with_db();
    let alice_cookie = register_and_login(&state).await;
    let alice_id = user_id_for(&state, "admin").await;

    seed_repository(&state, "legacy-repo", "/tmp/legacy-repo", &[&alice_id]).await;
    seed_workflow(&state, "LEG-1", &alice_id, "legacy-repo", None).await;

    let keys = list_workflow_keys(&state, &alice_cookie).await;
    assert_eq!(
        keys,
        vec!["LEG-1".to_string()],
        "workflow with repository_id=None should fall back to workspace_name → repositories.name match"
    );
}
