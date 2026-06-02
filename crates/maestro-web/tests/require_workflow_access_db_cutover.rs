// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! `require_workflow_access` is DB-first. These tests prove the path is
//! consulted by intentionally diverging the DB row from the in-memory
//! HashMap and observing that the response follows the DB.
//!
//! `require_workflow_access` is not exposed publicly, so we exercise
//! it through `GET /api/work-items/{key}` which calls it as its
//! first action.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use maestro_core::config::TicketingSystem;
use maestro_core::db::adapter::DbValue;
use maestro_core::db::repositories;
use maestro_core::workflow::engine::Workflow;

use maestro_web::server::build_router;
use maestro_web::state::AppState;
use maestro_web::test_helpers::{TEST_ORIGIN, register_and_login, test_state_with_db};

async fn create_and_login_user(state: &AppState, admin_cookie: &str, username: &str) -> String {
    let app = build_router(state.clone());
    let body =
        format!(r#"{{"username":"{username}","password":"testpassword1234","role":"user"}}"#);
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
    assert_eq!(resp.status(), StatusCode::CREATED);

    let app = build_router(state.clone());
    let body = format!(r#"{{"username":"{username}","password":"testpassword1234"}}"#);
    let resp = app
        .oneshot(
            Request::post("/api/auth/login")
                .header("Content-Type", "application/json")
                .header("Origin", TEST_ORIGIN)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let set_cookie = resp.headers().get("set-cookie").unwrap().to_str().unwrap();
    set_cookie.split(';').next().unwrap().trim().to_string()
}

async fn user_id_for(state: &AppState, username: &str) -> String {
    let db = state.auth().db.clone().expect("test state must have a DB");
    let user = maestro_core::db::users::get_user_by_username(db.adapter(), username)
        .await
        .expect("db query")
        .expect("user must exist");
    user.id
}

/// Seed a repo named `repo` and associate it with `user_id`.
async fn seed_repo(state: &AppState, name: &str, user_id: &str) -> String {
    let db = state.engine().engine.db().expect("db");
    let id = repositories::upsert(db.adapter(), name, None, "/tmp/ws", "main", None)
        .await
        .expect("repo upsert");
    repositories::add_for_user(db.adapter(), user_id, &id)
        .await
        .expect("add_for_user");
    id
}

/// Seed a workflow only in the in-memory HashMap (NO DB row).
/// Used to exercise the HashMap-fallback branch of the cutover.
async fn seed_workflow_in_map_only(
    state: &AppState,
    ticket_key: &str,
    user_id: &str,
    workspace_name: &str,
    repo_id: Option<String>,
) {
    let mut wf = Workflow::new(
        ticket_key.to_string(),
        "Summary".into(),
        false,
        false,
        TicketingSystem::None,
        None,
        workspace_name.to_string(),
    );
    wf.user_id = Some(user_id.to_string());
    wf.repository_id = repo_id;
    state
        .engine()
        .engine
        .workflows_arc()
        .write()
        .await
        .insert(ticket_key.to_string(), wf);
}

/// Seed a workflow in BOTH the HashMap and the DB. The two may
/// disagree on owner — useful for proving which one
/// `require_workflow_access` consults.
#[allow(clippy::too_many_arguments)]
async fn seed_workflow_in_map_and_db(
    state: &AppState,
    ticket_key: &str,
    map_owner: &str,
    map_repo_id: Option<String>,
    map_workspace: &str,
    db_owner: &str,
    db_repo_id: Option<String>,
    db_workspace: &str,
) {
    seed_workflow_in_map_only(state, ticket_key, map_owner, map_workspace, map_repo_id).await;

    let db = state.engine().engine.db().expect("db");
    db.adapter()
        .execute(
            "INSERT INTO work_items (\
                id, ticket_key, workspace_name, user_id, repository_id, private, \
                started_manually, counts_toward_manual_cap, driver_started, \
                jira_available, state_kind, started_at, created_at, updated_at\
             ) VALUES (?, ?, ?, ?, ?, 0, 0, 0, 0, 0, 'pending', 100, 100, 100)",
            vec![
                DbValue::Text(format!("uuid-{ticket_key}")),
                DbValue::Text(ticket_key.to_string()),
                DbValue::Text(db_workspace.to_string()),
                DbValue::Text(db_owner.to_string()),
                DbValue::TextOpt(db_repo_id),
            ],
        )
        .await
        .expect("insert work_items row");
}

async fn get_via_route(state: &AppState, ticket_key: &str, cookie: &str) -> StatusCode {
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::get(format!("/api/work-items/{ticket_key}"))
                .header("Cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    resp.status()
}

/// The cutover's load-bearing assertion: when DB and HashMap
/// disagree on ownership, the route follows the DB. Here the
/// HashMap claims bob owns it but the DB row says alice does —
/// alice gets access, bob does not.
#[tokio::test]
async fn db_row_wins_over_hashmap_on_ownership() {
    let state = test_state_with_db();
    let alice_cookie = register_and_login(&state).await;
    let alice_id = user_id_for(&state, "admin").await;
    let bob_cookie = create_and_login_user(&state, &alice_cookie, "bob").await;
    let bob_id = user_id_for(&state, "bob").await;

    // Seed a repo associated with alice (so alice's repo check
    // passes when we read from the DB row).
    let repo_alice = seed_repo(&state, "ws", &alice_id).await;
    let _repo_bob = seed_repo(&state, "ws-bob", &bob_id).await;

    seed_workflow_in_map_and_db(
        &state,
        "TICK-CUT",
        // HashMap claims bob is the owner...
        &bob_id,
        None,
        "ws-bob",
        // ...DB says alice is. The DB must win.
        &alice_id,
        Some(repo_alice),
        "ws",
    )
    .await;

    assert_eq!(
        get_via_route(&state, "TICK-CUT", &alice_cookie).await,
        StatusCode::OK,
        "alice (DB owner) must see the workflow"
    );
    assert_eq!(
        get_via_route(&state, "TICK-CUT", &bob_cookie).await,
        StatusCode::NOT_FOUND,
        "bob (HashMap owner only) must NOT see the workflow — DB is truth"
    );
}

/// When no DB row exists, the route falls back to the HashMap. Legacy
/// workflows that live only in memory must remain accessible to their owner.
#[tokio::test]
async fn hashmap_fallback_used_when_db_row_absent() {
    let state = test_state_with_db();
    let alice_cookie = register_and_login(&state).await;
    let alice_id = user_id_for(&state, "admin").await;
    let repo_alice = seed_repo(&state, "ws", &alice_id).await;

    // Workflow ONLY in the in-memory HashMap.
    seed_workflow_in_map_only(&state, "TICK-LEGACY", &alice_id, "ws", Some(repo_alice)).await;

    assert_eq!(
        get_via_route(&state, "TICK-LEGACY", &alice_cookie).await,
        StatusCode::OK,
        "owner of HashMap-only workflow must still see it (transition fallback)"
    );
}

/// When DB row's repo association points to a repo the caller has
/// NOT added, the cutover correctly rejects access — even if the
/// HashMap would have said yes.
#[tokio::test]
async fn db_repo_check_runs_against_db_row_not_hashmap() {
    let state = test_state_with_db();
    let alice_cookie = register_and_login(&state).await;
    let alice_id = user_id_for(&state, "admin").await;

    // Alice has added repo "ws".
    let repo_alice = seed_repo(&state, "ws", &alice_id).await;
    // Repo "stranger" exists but alice has not added it.
    let db = state.engine().engine.db().expect("db");
    let stranger_repo =
        repositories::upsert(db.adapter(), "stranger", None, "/tmp/strange", "main", None)
            .await
            .expect("repo upsert");

    seed_workflow_in_map_and_db(
        &state,
        "TICK-REPO",
        // HashMap (would-be permissive): alice owns + repo "ws" added.
        &alice_id,
        Some(repo_alice),
        "ws",
        // DB (authoritative): alice owns BUT repo is `stranger`.
        &alice_id,
        Some(stranger_repo),
        "stranger",
    )
    .await;

    assert_eq!(
        get_via_route(&state, "TICK-REPO", &alice_cookie).await,
        StatusCode::NOT_FOUND,
        "DB row's repository_id must drive the repo check, not the HashMap's"
    );
}
