// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Cross-endpoint regression guard for the workflow-state cutover read path:
//! when the in-memory cache and the authoritative `work_items` row disagree on
//! the durable fields, BOTH the list endpoint (`GET /api/work-items`) and the
//! single-item endpoint (`GET /api/work-items/{key}`) must serve the DB value.
//!
//! The per-field-group cutover tests (`*_scalars_*`, `*_pr_fields_*`) already
//! assert DB-wins for their fields on each endpoint individually; this test
//! pins that a single diverged work item resolves to the DB consistently
//! across both endpoints at once, so a future change can't make one path drift
//! back to the cache while the other still reads the DB.

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

async fn get_item(state: &AppState, key: &str, cookie: &str) -> serde_json::Value {
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::get(format!("/api/work-items/{key}"))
                .header("Cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "GET /api/work-items/{key}");
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn get_from_list(state: &AppState, key: &str, cookie: &str) -> serde_json::Value {
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
    assert_eq!(resp.status(), StatusCode::OK, "GET /api/work-items");
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let arr: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
    arr.into_iter()
        .find(|it| it["ticket_key"] == key)
        .unwrap_or_else(|| panic!("{key} missing from list"))
}

/// One work item, diverged on summary/description/type/branch/pr_url/pr_merged
/// between the cache and the DB row. Both read endpoints must return the DB
/// values, never the cache values.
#[tokio::test]
async fn list_and_get_both_serve_db_values_under_divergence() {
    let state = test_state_with_db();
    let cookie = register_and_login(&state).await;
    let uid = user_id_for(&state, "admin").await;
    let repo = seed_repo(&state, "ws", &uid).await;

    // Cache entry with "stale" values that must NOT surface.
    let mut wf = Workflow::new(
        "T-DIV".to_string(),
        "cache-summary".into(),
        false,
        false,
        TicketingSystem::None,
        None,
        "ws".into(),
    );
    wf.user_id = Some(uid.clone());
    wf.repository_id = Some(repo.clone());
    wf.ticket_description = "cache-desc".into();
    wf.ticket_type = "Bug".into();
    wf.branch_name = "feature/from-cache".into();
    wf.pr_url = Some("https://github.com/example/repo/pull/1".into());
    wf.pr_merged = false;
    state
        .engine()
        .engine
        .workflows_arc()
        .write()
        .await
        .insert("T-DIV".to_string(), wf);

    // Authoritative DB row with the values that MUST surface.
    let db = state.engine().engine.db().expect("db");
    db.adapter()
        .execute(
            "INSERT INTO work_items (\
                id, ticket_key, workspace_name, user_id, repository_id, private, \
                started_manually, counts_toward_manual_cap, driver_started, \
                jira_available, state_kind, started_at, created_at, updated_at, \
                ticket_summary, ticket_description, ticket_type, branch_name, \
                pr_url, pr_merged\
             ) VALUES ('uuid-T-DIV', 'T-DIV', 'ws', ?, ?, 0, 0, 0, 0, 0, 'pending', \
                100, 100, 100, 'db-summary', 'db-desc', 'Story', 'feature/from-db', \
                'https://github.com/example/repo/pull/42', 1)",
            vec![DbValue::Text(uid.clone()), DbValue::Text(repo.clone())],
        )
        .await
        .expect("insert work_items row");

    let expected = [
        ("ticket_summary", "db-summary"),
        ("ticket_description", "db-desc"),
        ("ticket_type", "Story"),
        ("branch_name", "feature/from-db"),
        ("pr_url", "https://github.com/example/repo/pull/42"),
    ];

    let via_get = get_item(&state, "T-DIV", &cookie).await;
    let via_list = get_from_list(&state, "T-DIV", &cookie).await;

    for (field, db_value) in expected {
        assert_eq!(via_get[field], db_value, "GET {field} must come from the DB");
        assert_eq!(
            via_list[field], db_value,
            "list {field} must come from the DB"
        );
    }
    assert_eq!(via_get["pr_merged"], true, "GET pr_merged must come from DB");
    assert_eq!(
        via_list["pr_merged"], true,
        "list pr_merged must come from DB"
    );
}
