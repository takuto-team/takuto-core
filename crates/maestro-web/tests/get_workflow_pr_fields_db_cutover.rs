// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! `get_workflow` reads `pr_url`, `pr_merged`, and `branch_name` from the
//! work_items row when present. These three scalars are written straight to
//! the DB by hooks (`update_pr_url` etc.) so the row is the freshest source.
//!
//! Tests diverge DB from HashMap on the three fields and confirm
//! the response follows the DB.

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
    // ── HashMap ───────────────────────────────────────────────
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

    // ── DB row ────────────────────────────────────────────────
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
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "GET /api/work-items/{ticket_key}"
    );
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// Load-bearing cutover assertion: DB diverges from HashMap on all
/// three scalar fields; the response follows the DB on every one.
#[tokio::test]
async fn pr_fields_come_from_db_row_not_hashmap() {
    let state = test_state_with_db();
    let cookie = register_and_login(&state).await;
    let uid = user_id_for(&state, "admin").await;
    let repo = seed_repo(&state, "ws", &uid).await;

    seed_map_and_db(
        &state,
        "T-PR",
        &uid,
        &repo,
        // HashMap (would-be-returned-before-cutover):
        "feature/from-map",
        Some("https://github.com/example/repo/pull/1"),
        false,
        // DB (authoritative):
        Some("feature/from-db"),
        Some("https://github.com/example/repo/pull/42"),
        true,
    )
    .await;

    let v = fetch_workflow(&state, "T-PR", &cookie).await;
    assert_eq!(
        v["branch_name"], "feature/from-db",
        "branch must come from DB"
    );
    assert_eq!(
        v["pr_url"], "https://github.com/example/repo/pull/42",
        "pr_url must come from DB"
    );
    assert_eq!(v["pr_merged"], true, "pr_merged must come from DB");
}

/// When the DB row has these fields set to NULL / 0 / NULL, the DB
/// is still authoritative — branch reports empty, pr_url is null,
/// pr_merged is false. We do NOT silently fall back to HashMap.
#[tokio::test]
async fn pr_fields_db_null_is_authoritative_not_fallback() {
    let state = test_state_with_db();
    let cookie = register_and_login(&state).await;
    let uid = user_id_for(&state, "admin").await;
    let repo = seed_repo(&state, "ws", &uid).await;

    seed_map_and_db(
        &state,
        "T-NULL",
        &uid,
        &repo,
        // HashMap claims a PR URL exists...
        "feature/from-map",
        Some("https://github.com/example/repo/pull/9"),
        true,
        // ...but the DB row has none. The DB is the source of truth.
        None,
        None,
        false,
    )
    .await;

    let v = fetch_workflow(&state, "T-NULL", &cookie).await;
    assert_eq!(v["branch_name"], "");
    assert!(
        v["pr_url"].is_null(),
        "pr_url must be null per DB, not the HashMap's URL"
    );
    assert_eq!(v["pr_merged"], false);
}

/// When no DB row exists, the HashMap values are used. Legacy
/// workflows must continue to render correctly when no work_items row
/// has been written yet.
#[tokio::test]
async fn pr_fields_fall_back_to_hashmap_when_no_db_row() {
    let state = test_state_with_db();
    let cookie = register_and_login(&state).await;
    let uid = user_id_for(&state, "admin").await;
    let repo = seed_repo(&state, "ws", &uid).await;

    // HashMap only.
    let mut wf = Workflow::new(
        "T-LEG".to_string(),
        "Summary".into(),
        false,
        false,
        TicketingSystem::None,
        None,
        "ws".into(),
    );
    wf.user_id = Some(uid.clone());
    wf.repository_id = Some(repo);
    wf.branch_name = "feature/legacy".into();
    wf.pr_url = Some("https://github.com/example/repo/pull/legacy".into());
    wf.pr_merged = true;
    state
        .engine()
        .engine
        .workflows_arc()
        .write()
        .await
        .insert("T-LEG".to_string(), wf);

    let v = fetch_workflow(&state, "T-LEG", &cookie).await;
    assert_eq!(v["branch_name"], "feature/legacy");
    assert_eq!(
        v["pr_url"], "https://github.com/example/repo/pull/legacy",
        "HashMap pr_url must be returned when no DB row"
    );
    assert_eq!(v["pr_merged"], true);
}
