// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Plan-07 slice 13 — `workflow_counts` aggregates over the DB,
//! with the in-memory HashMap as a transition fallback. Each entry
//! is keyed by ticket_key and the DB row wins when both sources
//! have data — so a workflow never double-counts during transition.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use maestro_core::config::TicketingSystem;
use maestro_core::db::adapter::DbValue;
use maestro_core::workflow::engine::Workflow;
use maestro_core::workflow::state::WorkflowState;

use maestro_web::server::build_router;
use maestro_web::state::AppState;
use maestro_web::test_helpers::{register_and_login, test_state_with_db};

async fn user_id_for(state: &AppState, username: &str) -> String {
    let db = state.auth().db.clone().expect("test state must have a DB");
    let user = maestro_core::db::users::get_user_by_username(db.adapter(), username)
        .await
        .expect("db query")
        .expect("user must exist");
    user.id
}

async fn seed_db_row(state: &AppState, ticket_key: &str, user_id: &str, state_kind: &str) {
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

async fn seed_map(state: &AppState, ticket_key: &str, user_id: &str, wf_state: WorkflowState) {
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
    wf.state = wf_state;
    state
        .engine()
        .engine
        .workflows_arc()
        .write()
        .await
        .insert(ticket_key.to_string(), wf);
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

/// DB rows alone drive the counts when the HashMap is empty.
#[tokio::test]
async fn counts_come_from_db_when_hashmap_empty() {
    let state = test_state_with_db();
    let alice_cookie = register_and_login(&state).await;
    let alice_id = user_id_for(&state, "admin").await;

    seed_db_row(&state, "T-DONE-1", &alice_id, "done").await;
    seed_db_row(&state, "T-DONE-2", &alice_id, "done").await;
    seed_db_row(&state, "T-RUNNING", &alice_id, "addressing_ticket").await;
    seed_db_row(&state, "T-PAUSED", &alice_id, "paused").await;
    seed_db_row(&state, "T-ERROR", &alice_id, "error").await;
    // Pending must NOT be counted (matches legacy behaviour).
    seed_db_row(&state, "T-PENDING", &alice_id, "pending").await;

    let v = fetch_counts(&state, &alice_cookie).await;
    assert_eq!(v["completed"], 2);
    assert_eq!(v["running"], 1);
    assert_eq!(v["paused"], 1);
    assert_eq!(v["errors"], 1);
}

/// HashMap fills in when the DB has nothing for this user.
#[tokio::test]
async fn counts_fall_back_to_hashmap_when_db_empty() {
    let state = test_state_with_db();
    let alice_cookie = register_and_login(&state).await;
    let alice_id = user_id_for(&state, "admin").await;

    seed_map(&state, "T-A", &alice_id, WorkflowState::Done).await;
    seed_map(&state, "T-B", &alice_id, WorkflowState::Stopped).await; // counts as error
    seed_map(
        &state,
        "T-C",
        &alice_id,
        WorkflowState::AddressingTicket { pass: 1 },
    )
    .await;

    let v = fetch_counts(&state, &alice_cookie).await;
    assert_eq!(v["completed"], 1);
    assert_eq!(v["errors"], 1);
    assert_eq!(v["running"], 1);
    assert_eq!(v["paused"], 0);
}

/// When both sources have the SAME ticket_key, the DB row wins and
/// the workflow is counted exactly once. This is the load-bearing
/// no-double-count assertion.
#[tokio::test]
async fn counts_dedupe_by_ticket_key_db_wins() {
    let state = test_state_with_db();
    let alice_cookie = register_and_login(&state).await;
    let alice_id = user_id_for(&state, "admin").await;

    // HashMap says Done, DB says Paused. DB must win → +1 paused, 0 completed.
    seed_map(&state, "T-DUP", &alice_id, WorkflowState::Done).await;
    seed_db_row(&state, "T-DUP", &alice_id, "paused").await;

    let v = fetch_counts(&state, &alice_cookie).await;
    assert_eq!(v["completed"], 0, "HashMap's Done must be overridden by DB's Paused");
    assert_eq!(v["paused"], 1, "DB row wins on dedupe");
    assert_eq!(v["running"], 0);
    assert_eq!(v["errors"], 0);
}

/// Other users' rows are never counted, regardless of source.
#[tokio::test]
async fn counts_scope_to_caller_only() {
    let state = test_state_with_db();
    let alice_cookie = register_and_login(&state).await;
    let alice_id = user_id_for(&state, "admin").await;

    // Seed a second user so the FK on work_items.user_id is satisfied.
    let db = state.engine().engine.db().expect("db");
    db.adapter()
        .execute(
            "INSERT INTO users (id, username, role) VALUES ('bob-uid', 'bob', 'user')",
            Vec::<DbValue>::new(),
        )
        .await
        .expect("seed bob");

    // Seed a row for bob; counts for alice must exclude it.
    seed_db_row(&state, "T-BOB", "bob-uid", "done").await;
    seed_db_row(&state, "T-ALICE", &alice_id, "done").await;

    let v = fetch_counts(&state, &alice_cookie).await;
    assert_eq!(v["completed"], 1);
    assert_eq!(v["running"], 0);
    assert_eq!(v["paused"], 0);
    assert_eq!(v["errors"], 0);
}
