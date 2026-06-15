// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! `workflow_counts` mirrors the dashboard grid: it counts the caller's
//! in-memory workflows in repositories they've added (same source + same repo
//! gate as `list_workflows`), tallying the live in-memory state. DB rows that
//! are not on the dashboard never inflate the summary bar.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use takuto_core::config::TicketingSystem;
use takuto_core::db::adapter::DbValue;
use takuto_core::db::repositories;
use takuto_core::workflow::engine::Workflow;
use takuto_core::workflow::state::WorkflowState;

use takuto_web::server::build_router;
use takuto_web::state::AppState;
use takuto_web::test_helpers::{register_and_login, test_state_with_db};

async fn user_id_for(state: &AppState, username: &str) -> String {
    let db = state.auth().db.clone().expect("test state must have a DB");
    let user = takuto_core::db::users::get_user_by_username(db.adapter(), username)
        .await
        .expect("db query")
        .expect("user must exist");
    user.id
}

/// Create a repository and associate it with the user so workflows in it pass
/// the grid's visibility gate. Returns the repo id.
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

/// Insert an in-memory workflow tagged with the given owner + repository.
async fn seed_map(
    state: &AppState,
    ticket_key: &str,
    user_id: &str,
    repo_id: &str,
    wf_state: WorkflowState,
) {
    let mut wf = Workflow::new(
        ticket_key.to_string(),
        "Summary".into(),
        false,
        false,
        TicketingSystem::None,
        None,
        "ws".into(),
    );
    wf.user_id = Some(user_id.to_string());
    wf.repository_id = Some(repo_id.to_string());
    wf.state = wf_state;
    state
        .engine()
        .engine
        .workflows_arc()
        .write()
        .await
        .insert(ticket_key.to_string(), wf);
}

/// Insert a DB-only work_items row (no in-memory workflow). Such a row is NOT
/// on the dashboard and must not be counted.
async fn seed_db_only_row(state: &AppState, ticket_key: &str, user_id: &str, state_kind: &str) {
    let db = state.engine().engine.db().expect("db");
    db.adapter()
        .execute(
            "INSERT INTO work_items (\
                id, ticket_key, workspace_name, user_id, private, \
                started_manually, counts_toward_manual_cap, driver_started, \
                jira_available, state_kind, started_at, created_at, updated_at\
             ) VALUES (?, ?, 'ws', ?, 0, 0, 0, 0, 0, ?, 100, 100, 100)",
            vec![
                DbValue::Text(format!("uuid-{ticket_key}")),
                DbValue::Text(ticket_key.to_string()),
                DbValue::Text(user_id.to_string()),
                DbValue::Text(state_kind.to_string()),
            ],
        )
        .await
        .expect("insert work_items row");
}

async fn fetch_counts(state: &AppState, cookie: &str) -> serde_json::Value {
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::get("/api/work-items/counts")
                .header("Cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// Counts reflect the in-memory workflows the grid shows; Pending is not
/// counted in any active bucket.
#[tokio::test]
async fn counts_come_from_in_memory_map() {
    let state = test_state_with_db();
    let alice_cookie = register_and_login(&state).await;
    let alice_id = user_id_for(&state, "admin").await;
    let repo = seed_repo(&state, "ws", &alice_id).await;

    seed_map(&state, "T-DONE-1", &alice_id, &repo, WorkflowState::Done).await;
    seed_map(&state, "T-DONE-2", &alice_id, &repo, WorkflowState::Done).await;
    seed_map(
        &state,
        "T-RUNNING",
        &alice_id,
        &repo,
        WorkflowState::AddressingTicket { pass: 1 },
    )
    .await;
    seed_map(
        &state,
        "T-PAUSED",
        &alice_id,
        &repo,
        WorkflowState::Paused {
            source_state: Box::new(WorkflowState::AddressingTicket { pass: 1 }),
        },
    )
    .await;
    seed_map(
        &state,
        "T-ERROR",
        &alice_id,
        &repo,
        WorkflowState::Error {
            source_state: Box::new(WorkflowState::AddressingTicket { pass: 1 }),
            message: "boom".into(),
        },
    )
    .await;
    // Stopped is reported in the "errors" bucket.
    seed_map(
        &state,
        "T-STOPPED",
        &alice_id,
        &repo,
        WorkflowState::Stopped,
    )
    .await;
    // Pending must NOT be counted in any bucket.
    seed_map(
        &state,
        "T-PENDING",
        &alice_id,
        &repo,
        WorkflowState::Pending,
    )
    .await;

    let v = fetch_counts(&state, &alice_cookie).await;
    assert_eq!(v["completed"], 2);
    assert_eq!(v["running"], 1);
    assert_eq!(v["paused"], 1);
    assert_eq!(v["errors"], 2, "Error + Stopped both land in errors");
}

/// The reported bug: a DB work_items row with no in-memory workflow is NOT on
/// the dashboard and must NOT inflate the counters.
#[tokio::test]
async fn counts_ignore_db_only_rows_not_on_dashboard() {
    let state = test_state_with_db();
    let alice_cookie = register_and_login(&state).await;
    let alice_id = user_id_for(&state, "admin").await;
    let repo = seed_repo(&state, "ws", &alice_id).await;

    // One real card on the dashboard…
    seed_map(
        &state,
        "T-RUNNING",
        &alice_id,
        &repo,
        WorkflowState::AddressingTicket { pass: 1 },
    )
    .await;
    // …and stale DB-only rows that are NOT in the map (e.g. another workspace
    // or a not-restored terminal row). These must be ignored.
    seed_db_only_row(&state, "T-GHOST-ERR-1", &alice_id, "error").await;
    seed_db_only_row(&state, "T-GHOST-ERR-2", &alice_id, "error").await;
    seed_db_only_row(&state, "T-GHOST-DONE", &alice_id, "done").await;

    let v = fetch_counts(&state, &alice_cookie).await;
    assert_eq!(v["running"], 1, "the one in-memory card counts");
    assert_eq!(v["errors"], 0, "stale DB-only error rows must NOT count");
    assert_eq!(v["completed"], 0);
    assert_eq!(v["paused"], 0);
}

/// A workflow in a repository the caller has NOT added is not visible on the
/// grid, so it is not counted either.
#[tokio::test]
async fn counts_exclude_workflows_in_unadded_repos() {
    let state = test_state_with_db();
    let alice_cookie = register_and_login(&state).await;
    let alice_id = user_id_for(&state, "admin").await;
    let added = seed_repo(&state, "ws", &alice_id).await;

    // Create a second repo but do NOT add it to alice.
    let db = state.engine().engine.db().expect("db");
    let other = repositories::upsert(db.adapter(), "other-ws", None, "/tmp/other", "main", None)
        .await
        .expect("repo upsert");

    seed_map(&state, "T-VISIBLE", &alice_id, &added, WorkflowState::Done).await;
    // The hidden workflow lives in the unadded repo AND carries that repo's
    // workspace_name, so neither the repository_id gate nor the name-fallback
    // makes it visible.
    {
        let mut wf = Workflow::new(
            "T-HIDDEN".to_string(),
            "Summary".into(),
            false,
            false,
            TicketingSystem::None,
            None,
            "other-ws".into(),
        );
        wf.user_id = Some(alice_id.clone());
        wf.repository_id = Some(other.clone());
        wf.state = WorkflowState::Error {
            source_state: Box::new(WorkflowState::AddressingTicket { pass: 1 }),
            message: "boom".into(),
        };
        state
            .engine()
            .engine
            .workflows_arc()
            .write()
            .await
            .insert("T-HIDDEN".to_string(), wf);
    }

    let v = fetch_counts(&state, &alice_cookie).await;
    assert_eq!(v["completed"], 1);
    assert_eq!(v["errors"], 0, "workflow in an unadded repo must not count");
}

/// Other users' workflows are never counted.
#[tokio::test]
async fn counts_scope_to_caller_only() {
    let state = test_state_with_db();
    let alice_cookie = register_and_login(&state).await;
    let alice_id = user_id_for(&state, "admin").await;
    let repo = seed_repo(&state, "ws", &alice_id).await;

    // Seed a second user and associate them with the same repo.
    let db = state.engine().engine.db().expect("db");
    db.adapter()
        .execute(
            "INSERT INTO users (id, username, role) VALUES ('bob-uid', 'bob', 'user')",
            Vec::<DbValue>::new(),
        )
        .await
        .expect("seed bob");
    repositories::add_for_user(db.adapter(), "bob-uid", &repo)
        .await
        .expect("add bob");

    seed_map(&state, "T-ALICE", &alice_id, &repo, WorkflowState::Done).await;
    seed_map(&state, "T-BOB", "bob-uid", &repo, WorkflowState::Done).await;

    let v = fetch_counts(&state, &alice_cookie).await;
    assert_eq!(v["completed"], 1, "only alice's workflow counts");
    assert_eq!(v["running"], 0);
    assert_eq!(v["paused"], 0);
    assert_eq!(v["errors"], 0);
}
