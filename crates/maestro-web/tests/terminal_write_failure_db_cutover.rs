// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! End-to-end guard for the fail-loud terminal-state write (workflow-state
//! cutover invariant I4): a DB failure during a terminal transition must not
//! block the user action or corrupt the in-memory cache.
//!
//! Fault injection: we drop the `current_step_label` column — which
//! `update_work_item_state` writes but the ownership read
//! (`get_access_fields_by_ticket_key`) does NOT select — so the request's
//! access check still succeeds while ONLY the terminal write fails. Stopping
//! the workflow over HTTP then exercises the fail-loud path
//! (`shadow_persist_state_change` for the terminal `Stopped` state).
//!
//! What this asserts end-to-end: the stop still returns 200 (the failure is
//! surfaced via retry+ERROR logging, not propagated as a request error), the
//! cache transitions to `Stopped`, and the DB row's state is left unchanged by
//! the failed write (proving the failure path was genuinely exercised, not a
//! silent no-op). The fail-loud *surfacing* itself — bounded retry then an
//! ERROR rather than the old silent WARN — is unit-tested in
//! `workflow::engine::driver::tests::retry_durable_write_*`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use maestro_core::config::TicketingSystem;
use maestro_core::db::adapter::DbValue;
use maestro_core::db::repositories;
use maestro_core::workflow::engine::Workflow;
use maestro_core::workflow::state::WorkflowState;

use maestro_web::server::build_router;
use maestro_web::state::AppState;
use maestro_web::test_helpers::{TEST_ORIGIN, register_and_login, test_state_with_db};

async fn user_id_for(state: &AppState, username: &str) -> String {
    let db = state.auth().db.clone().expect("test state must have a DB");
    maestro_core::db::users::get_user_by_username(db.adapter(), username)
        .await
        .expect("db query")
        .expect("user must exist")
        .id
}

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

#[tokio::test]
async fn terminal_write_failure_does_not_block_stop_or_corrupt_cache() {
    let state = test_state_with_db();
    let cookie = register_and_login(&state).await;
    let uid = user_id_for(&state, "admin").await;
    let repo = seed_repo(&state, "ws", &uid).await;

    // Cache entry in an active (running) state — eligible to be stopped.
    let mut wf = Workflow::new(
        "T-STOP".to_string(),
        "Summary".into(),
        false,
        false,
        TicketingSystem::None,
        None,
        "ws".into(),
    );
    wf.user_id = Some(uid.clone());
    wf.repository_id = Some(repo.clone());
    wf.state = WorkflowState::AddressingTicket { pass: 1 };
    state
        .engine()
        .engine
        .workflows_arc()
        .write()
        .await
        .insert("T-STOP".to_string(), wf);

    // Authoritative DB row so the ownership read in `require_workflow_access`
    // succeeds.
    let db = state.engine().engine.db().expect("db");
    db.adapter()
        .execute(
            "INSERT INTO work_items (\
                id, ticket_key, workspace_name, user_id, repository_id, private, \
                started_manually, counts_toward_manual_cap, driver_started, \
                jira_available, state_kind, started_at, created_at, updated_at\
             ) VALUES ('uuid-T-STOP', 'T-STOP', 'ws', ?, ?, 0, 0, 0, 1, 0, \
                'addressing_ticket', 100, 100, 100)",
            vec![DbValue::Text(uid.clone()), DbValue::Text(repo.clone())],
        )
        .await
        .expect("insert work_items row");

    // Fault injection: drop a column the terminal write SETS but the ownership
    // read does NOT select, so only `update_work_item_state` fails.
    db.adapter()
        .execute(
            "ALTER TABLE work_items DROP COLUMN current_step_label",
            vec![],
        )
        .await
        .expect("drop current_step_label column");

    // Stop the workflow through the HTTP layer.
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::post("/api/work-items/T-STOP/stop")
                .header("Cookie", &cookie)
                .header("Origin", TEST_ORIGIN)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "stop must complete despite the failing terminal write"
    );

    // The cache transitioned to the terminal Stopped state.
    let stopped = {
        let arc = state.engine().engine.workflows_arc();
        let map = arc.read().await;
        matches!(map.get("T-STOP"), Some(w) if matches!(w.state, WorkflowState::Stopped))
    };
    assert!(stopped, "cache must reflect the terminal Stopped state");

    // The terminal write genuinely failed: the DB row's state_kind is
    // unchanged. Read state_kind directly — a full row decode would touch the
    // dropped column.
    let row = db
        .adapter()
        .query_one(
            "SELECT state_kind FROM work_items WHERE id = 'uuid-T-STOP'",
            vec![],
        )
        .await
        .expect("read state_kind");
    assert_eq!(
        row.get_text(0).expect("state_kind"),
        "addressing_ticket",
        "the failed terminal write must not have mutated the DB row"
    );
}
