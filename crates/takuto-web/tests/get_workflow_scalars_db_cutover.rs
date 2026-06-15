// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Verify that `get_workflow` reads its shadow-written scalar
//! fields (ticket summary/description/type, timestamps, worktree
//! path) from the `work_items` row instead of the in-memory
//! `Workflow`. PR fields are covered by a sibling test file.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use takuto_core::config::TicketingSystem;
use takuto_core::db::adapter::DbValue;
use takuto_core::db::repositories;
use takuto_core::workflow::engine::Workflow;

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

async fn fetch_workflow(state: &AppState, ticket_key: &str, cookie: &str) -> serde_json::Value {
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
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// Seed a workflow in BOTH the in-memory map AND the DB. The two
/// sources carry different values for the scalar fields so the
/// test can prove which one the route returns.
#[allow(clippy::too_many_arguments)]
async fn seed_diverged(
    state: &AppState,
    ticket_key: &str,
    user_id: &str,
    repo_id: &str,
    map_summary: &str,
    map_description: &str,
    map_type: &str,
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
    wf.ticket_description = map_description.to_string();
    wf.ticket_type = map_type.to_string();
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

/// Load-bearing assertion: every shadow-written scalar field
/// returned by `get_workflow` follows the DB row, not the in-memory
/// Workflow, when both sources exist.
#[tokio::test]
async fn scalar_fields_come_from_db_row_not_hashmap() {
    let state = test_state_with_db();
    let cookie = register_and_login(&state).await;
    let uid = user_id_for(&state, "admin").await;
    let repo = seed_repo(&state, "ws", &uid).await;

    // Create a worktree dir on disk so the path filter doesn't drop it.
    let tmp = std::env::temp_dir().join(format!("takuto-cw-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp).unwrap();
    let wt = tmp.join("from-db");
    std::fs::create_dir_all(&wt).unwrap();

    seed_diverged(
        &state,
        "T-SCALAR",
        &uid,
        &repo,
        "map summary",
        "map description",
        "Bug",
        Some("db summary"),
        Some("db description"),
        Some("Story"),
        Some(wt.to_str().unwrap()),
        1_700_000_000,
        1_700_000_500,
    )
    .await;

    let v = fetch_workflow(&state, "T-SCALAR", &cookie).await;
    assert_eq!(v["ticket_summary"], "db summary");
    assert_eq!(v["ticket_description"], "db description");
    assert_eq!(v["ticket_type"], "Story");
    assert_eq!(v["worktree_path"], wt.to_str().unwrap());
    // RFC3339 with millisecond precision; check the prefix to avoid
    // brittleness around timezone formatting.
    assert!(
        v["started_at"].as_str().unwrap().starts_with("2023-11-14"),
        "started_at must reflect the DB's 1_700_000_000 unix, got {}",
        v["started_at"]
    );
    assert!(
        v["updated_at"].as_str().unwrap().starts_with("2023-11-14"),
        "updated_at must reflect the DB unix, got {}",
        v["updated_at"]
    );

    std::fs::remove_dir_all(&tmp).ok();
}

/// When the DB row has these scalars set to NULL, the DB is still
/// authoritative — fields read as empty strings (matching the
/// existing convention for unset scalar text). We do NOT silently
/// fall back to the in-memory values.
#[tokio::test]
async fn db_null_is_authoritative_not_fallback() {
    let state = test_state_with_db();
    let cookie = register_and_login(&state).await;
    let uid = user_id_for(&state, "admin").await;
    let repo = seed_repo(&state, "ws", &uid).await;

    seed_diverged(
        &state,
        "T-NULL",
        &uid,
        &repo,
        "map says here",
        "map description",
        "Bug",
        None,
        None,
        None,
        None,
        100,
        100,
    )
    .await;

    let v = fetch_workflow(&state, "T-NULL", &cookie).await;
    assert_eq!(
        v["ticket_summary"], "",
        "DB NULL beats in-memory 'map says here'"
    );
    assert_eq!(v["ticket_description"], "");
    assert_eq!(v["ticket_type"], "");
    assert!(v["worktree_path"].is_null());
}

/// When no DB row exists, the in-memory `Workflow` is used. Legacy
/// items must continue to render during the transition window.
#[tokio::test]
async fn falls_back_to_hashmap_when_no_db_row() {
    let state = test_state_with_db();
    let cookie = register_and_login(&state).await;
    let uid = user_id_for(&state, "admin").await;
    let repo = seed_repo(&state, "ws", &uid).await;

    let mut wf = Workflow::new(
        "T-LEG".to_string(),
        "legacy summary".into(),
        false,
        false,
        TicketingSystem::None,
        None,
        "ws".into(),
    );
    wf.user_id = Some(uid.clone());
    wf.repository_id = Some(repo);
    wf.ticket_description = "legacy description".into();
    wf.ticket_type = "Task".into();
    state
        .engine()
        .engine
        .workflows_arc()
        .write()
        .await
        .insert("T-LEG".to_string(), wf);

    let v = fetch_workflow(&state, "T-LEG", &cookie).await;
    assert_eq!(v["ticket_summary"], "legacy summary");
    assert_eq!(v["ticket_description"], "legacy description");
    assert_eq!(v["ticket_type"], "Task");
}
