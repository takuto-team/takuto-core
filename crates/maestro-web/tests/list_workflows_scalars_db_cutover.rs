// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Verify that `list_workflows` reads its shadow-written scalar
//! fields (ticket summary/description/type, timestamps, worktree
//! path) from the `work_items` row instead of the in-memory
//! `Workflow`, applied per-row in the projection.

use std::collections::HashMap;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use maestro_core::config::TicketingSystem;
use maestro_core::db::adapter::DbValue;
use maestro_core::db::repositories;
use maestro_core::workflow::engine::Workflow;

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

#[allow(clippy::too_many_arguments)]
async fn seed_diverged(
    state: &AppState,
    ticket_key: &str,
    user_id: &str,
    repo_id: &str,
    map_summary: &str,
    db_summary: Option<&str>,
    db_description: Option<&str>,
    db_type: Option<&str>,
    db_worktree: Option<&str>,
    db_started_at_unix: i64,
    db_updated_at_unix: i64,
) {
    let mut wf = Workflow::new(
        ticket_key.to_string(),
        map_summary.to_string(),
        false,
        false,
        TicketingSystem::None,
        None,
        "ws".into(),
    );
    wf.user_id = Some(user_id.to_string());
    wf.repository_id = Some(repo_id.to_string());
    wf.ticket_description = "map description".into();
    wf.ticket_type = "Bug".into();
    state
        .engine()
        .engine
        .workflows_arc()
        .write()
        .await
        .insert(ticket_key.to_string(), wf);

    let db = state.engine().engine.db().expect("db");
    db.adapter()
        .execute(
            "INSERT INTO work_items (\
                id, ticket_key, workspace_name, user_id, repository_id, private, \
                started_manually, counts_toward_manual_cap, driver_started, \
                jira_available, state_kind, started_at, created_at, updated_at, \
                ticket_summary, ticket_description, ticket_type, worktree_path\
             ) VALUES (?, ?, 'ws', ?, ?, 0, 0, 0, 0, 0, 'pending', ?, ?, ?, ?, ?, ?, ?)",
            vec![
                DbValue::Text(format!("uuid-{ticket_key}")),
                DbValue::Text(ticket_key.to_string()),
                DbValue::Text(user_id.to_string()),
                DbValue::Text(repo_id.to_string()),
                DbValue::I64(db_started_at_unix),
                DbValue::I64(db_started_at_unix),
                DbValue::I64(db_updated_at_unix),
                DbValue::TextOpt(db_summary.map(str::to_string)),
                DbValue::TextOpt(db_description.map(str::to_string)),
                DbValue::TextOpt(db_type.map(str::to_string)),
                DbValue::TextOpt(db_worktree.map(str::to_string)),
            ],
        )
        .await
        .expect("insert work_items row");
}

async fn fetch_list_by_key(state: &AppState, cookie: &str) -> HashMap<String, serde_json::Value> {
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::get("/api/work-items")
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
    let arr: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
    arr.into_iter()
        .map(|item| (item["ticket_key"].as_str().unwrap().to_string(), item))
        .collect()
}

/// Across multiple workflows, each one's shadow-written scalars
/// follow its DB row, not the in-memory Workflow.
#[tokio::test]
async fn list_scalar_fields_come_from_db_rows() {
    let state = test_state_with_db();
    let cookie = register_and_login(&state).await;
    let uid = user_id_for(&state, "admin").await;
    let repo = seed_repo(&state, "ws", &uid).await;

    let tmp = std::env::temp_dir().join(format!("maestro-list-{}", uuid::Uuid::new_v4()));
    let wt1 = tmp.join("a");
    let wt2 = tmp.join("b");
    std::fs::create_dir_all(&wt1).unwrap();
    std::fs::create_dir_all(&wt2).unwrap();

    seed_diverged(
        &state,
        "T-1",
        &uid,
        &repo,
        "map-summary-1",
        Some("db-summary-1"),
        Some("db-desc-1"),
        Some("Story"),
        Some(wt1.to_str().unwrap()),
        1_700_000_000,
        1_700_000_500,
    )
    .await;
    seed_diverged(
        &state,
        "T-2",
        &uid,
        &repo,
        "map-summary-2",
        Some("db-summary-2"),
        Some("db-desc-2"),
        Some("Task"),
        Some(wt2.to_str().unwrap()),
        1_700_100_000,
        1_700_100_500,
    )
    .await;

    let by_key = fetch_list_by_key(&state, &cookie).await;
    assert_eq!(by_key["T-1"]["ticket_summary"], "db-summary-1");
    assert_eq!(by_key["T-1"]["ticket_description"], "db-desc-1");
    assert_eq!(by_key["T-1"]["ticket_type"], "Story");
    assert_eq!(by_key["T-1"]["worktree_path"], wt1.to_str().unwrap());
    assert_eq!(by_key["T-2"]["ticket_summary"], "db-summary-2");
    assert_eq!(by_key["T-2"]["ticket_type"], "Task");
    assert_eq!(by_key["T-2"]["worktree_path"], wt2.to_str().unwrap());

    std::fs::remove_dir_all(&tmp).ok();
}

/// Mixed list: a row with a DB entry alongside one without. Each
/// projects from the correct source.
#[tokio::test]
async fn list_falls_back_per_row_when_db_row_missing() {
    let state = test_state_with_db();
    let cookie = register_and_login(&state).await;
    let uid = user_id_for(&state, "admin").await;
    let repo = seed_repo(&state, "ws", &uid).await;

    seed_diverged(
        &state,
        "T-A",
        &uid,
        &repo,
        "map-A",
        Some("db-A"),
        Some("db-A-desc"),
        Some("Story"),
        None,
        100,
        100,
    )
    .await;

    let mut wf = Workflow::new(
        "T-B".to_string(),
        "in-memory-summary".into(),
        false,
        false,
        TicketingSystem::None,
        None,
        "ws".into(),
    );
    wf.user_id = Some(uid.clone());
    wf.repository_id = Some(repo);
    wf.ticket_description = "in-memory-desc".into();
    wf.ticket_type = "Bug".into();
    state
        .engine()
        .engine
        .workflows_arc()
        .write()
        .await
        .insert("T-B".to_string(), wf);

    let by_key = fetch_list_by_key(&state, &cookie).await;
    assert_eq!(by_key["T-A"]["ticket_summary"], "db-A");
    assert_eq!(by_key["T-A"]["ticket_type"], "Story");
    assert_eq!(by_key["T-B"]["ticket_summary"], "in-memory-summary");
    assert_eq!(by_key["T-B"]["ticket_description"], "in-memory-desc");
    assert_eq!(by_key["T-B"]["ticket_type"], "Bug");
}
