// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! `list_workflows` reads pr_url / pr_merged / branch_name from work_items
//! rows (batched), mirroring the single-item `get_workflow` behaviour but
//! applied per-item across the dashboard list.

use std::collections::HashMap;

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

#[allow(clippy::too_many_arguments)]
async fn seed_map_and_db(
    state: &AppState,
    ticket_key: &str,
    user_id: &str,
    repo_id: &str,
    map_branch: &str,
    map_pr_url: Option<&str>,
    map_pr_merged: bool,
    db_branch: Option<&str>,
    db_pr_url: Option<&str>,
    db_pr_merged: bool,
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
    wf.branch_name = map_branch.to_string();
    wf.pr_url = map_pr_url.map(str::to_string);
    wf.pr_merged = map_pr_merged;
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
                branch_name, pr_url, pr_merged\
             ) VALUES (?, ?, 'ws', ?, ?, 0, 0, 0, 0, 0, 'pending', 100, 100, 100, ?, ?, ?)",
            vec![
                DbValue::Text(format!("uuid-{ticket_key}")),
                DbValue::Text(ticket_key.to_string()),
                DbValue::Text(user_id.to_string()),
                DbValue::Text(repo_id.to_string()),
                DbValue::TextOpt(db_branch.map(str::to_string)),
                DbValue::TextOpt(db_pr_url.map(str::to_string)),
                DbValue::I64(db_pr_merged.into()),
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

/// Load-bearing cutover assertion for the list endpoint: across
/// multiple workflows, each one's pr_url / pr_merged / branch_name
/// follows the DB row, not the HashMap.
#[tokio::test]
async fn list_pr_fields_come_from_db_rows() {
    let state = test_state_with_db();
    let cookie = register_and_login(&state).await;
    let uid = user_id_for(&state, "admin").await;
    let repo = seed_repo(&state, "ws", &uid).await;

    seed_map_and_db(
        &state,
        "T-LIST-1",
        &uid,
        &repo,
        "feature/m1",
        Some("https://github.com/example/repo/pull/1"),
        false,
        Some("feature/db-1"),
        Some("https://github.com/example/repo/pull/100"),
        true,
    )
    .await;
    seed_map_and_db(
        &state,
        "T-LIST-2",
        &uid,
        &repo,
        "feature/m2",
        Some("https://github.com/example/repo/pull/2"),
        true,
        Some("feature/db-2"),
        Some("https://github.com/example/repo/pull/200"),
        false,
    )
    .await;

    let by_key = fetch_list_by_key(&state, &cookie).await;
    let one = &by_key["T-LIST-1"];
    assert_eq!(one["branch_name"], "feature/db-1");
    assert_eq!(one["pr_url"], "https://github.com/example/repo/pull/100");
    assert_eq!(one["pr_merged"], true);
    let two = &by_key["T-LIST-2"];
    assert_eq!(two["branch_name"], "feature/db-2");
    assert_eq!(two["pr_url"], "https://github.com/example/repo/pull/200");
    assert_eq!(two["pr_merged"], false);
}

/// A list with mixed DB-backed and HashMap-only workflows must
/// project the correct source per row. Backfilled rows use DB,
/// transitional in-memory-only rows use HashMap.
#[tokio::test]
async fn list_falls_back_per_row_when_db_row_missing() {
    let state = test_state_with_db();
    let cookie = register_and_login(&state).await;
    let uid = user_id_for(&state, "admin").await;
    let repo = seed_repo(&state, "ws", &uid).await;

    // T-A: both map and DB (DB wins).
    seed_map_and_db(
        &state,
        "T-A",
        &uid,
        &repo,
        "map-branch",
        Some("https://map"),
        false,
        Some("db-branch"),
        Some("https://db"),
        true,
    )
    .await;

    // T-B: HashMap only.
    let mut wf = Workflow::new(
        "T-B".to_string(),
        "Summary".into(),
        false,
        false,
        TicketingSystem::None,
        None,
        "ws".into(),
    );
    wf.user_id = Some(uid.clone());
    wf.repository_id = Some(repo);
    wf.branch_name = "legacy-branch".into();
    wf.pr_url = Some("https://legacy".into());
    wf.pr_merged = true;
    state
        .engine()
        .engine
        .workflows_arc()
        .write()
        .await
        .insert("T-B".to_string(), wf);

    let by_key = fetch_list_by_key(&state, &cookie).await;
    assert_eq!(by_key["T-A"]["branch_name"], "db-branch");
    assert_eq!(by_key["T-A"]["pr_url"], "https://db");
    assert_eq!(by_key["T-A"]["pr_merged"], true);
    assert_eq!(by_key["T-B"]["branch_name"], "legacy-branch");
    assert_eq!(by_key["T-B"]["pr_url"], "https://legacy");
    assert_eq!(by_key["T-B"]["pr_merged"], true);
}
