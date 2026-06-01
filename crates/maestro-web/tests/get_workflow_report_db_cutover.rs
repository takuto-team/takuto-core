// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Plan-07 slice 12 — `get_workflow_report` reads `worktree_path` +
//! `ticket_key` from the work_items row. Tests prove the DB path is
//! consulted by intentionally diverging the DB row's worktree_path
//! from the HashMap's: the response must follow the DB row (or the
//! HashMap when no row exists).

use std::io::Write;

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

/// Write `lore/reports/<ticket_key>_report.md` under the worktree
/// with deterministic contents that include the worktree's
/// directory name — so the test can prove which worktree the
/// route actually opened.
fn write_report(worktree: &std::path::Path, ticket_key: &str, marker: &str) {
    let reports_dir = worktree.join("lore").join("reports");
    std::fs::create_dir_all(&reports_dir).unwrap();
    let report_path = reports_dir.join(format!("{ticket_key}_report.md"));
    let mut f = std::fs::File::create(&report_path).unwrap();
    writeln!(f, "report-marker:{marker}").unwrap();
}

async fn seed_workflow_in_map(
    state: &AppState,
    ticket_key: &str,
    user_id: &str,
    workspace_name: &str,
    repo_id: Option<String>,
    worktree_path: std::path::PathBuf,
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
    wf.worktree_path = Some(worktree_path);
    state
        .engine()
        .engine
        .workflows_arc()
        .write()
        .await
        .insert(ticket_key.to_string(), wf);
}

#[allow(clippy::too_many_arguments)]
async fn seed_db_row(
    state: &AppState,
    ticket_key: &str,
    user_id: &str,
    workspace_name: &str,
    repo_id: Option<String>,
    worktree_path: &std::path::Path,
) {
    let db = state.engine().engine.db().expect("db");
    db.adapter()
        .execute(
            "INSERT INTO work_items (\
                id, ticket_key, workspace_name, user_id, repository_id, private, \
                started_manually, counts_toward_manual_cap, driver_started, \
                jira_available, state_kind, started_at, created_at, updated_at, \
                worktree_path\
             ) VALUES (?, ?, ?, ?, ?, 0, 0, 0, 0, 0, 'pending', 100, 100, 100, ?)",
            vec![
                DbValue::Text(format!("uuid-{ticket_key}")),
                DbValue::Text(ticket_key.to_string()),
                DbValue::Text(workspace_name.to_string()),
                DbValue::Text(user_id.to_string()),
                DbValue::TextOpt(repo_id),
                DbValue::Text(worktree_path.display().to_string()),
            ],
        )
        .await
        .expect("insert work_items row");
}

async fn fetch_report(state: &AppState, ticket_key: &str, cookie: &str) -> (StatusCode, String) {
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::get(format!("/api/work-items/{ticket_key}/report"))
                .header("Cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

/// Load-bearing cutover assertion: when DB and HashMap point at
/// different worktree paths, the route opens the DB's. Both
/// worktrees exist on disk with different marker contents so we
/// can tell which one the handler read.
#[tokio::test]
async fn report_reads_worktree_from_db_row_not_hashmap() {
    let state = test_state_with_db();
    let alice_cookie = register_and_login(&state).await;
    let alice_id = user_id_for(&state, "admin").await;
    let repo_id = seed_repo(&state, "ws", &alice_id).await;

    // Two distinct on-disk worktrees with distinct marker files.
    let tmp = std::env::temp_dir().join(format!("maestro-rep-{}", uuid::Uuid::new_v4()));
    let map_wt = tmp.join("map-worktree");
    let db_wt = tmp.join("db-worktree");
    std::fs::create_dir_all(&map_wt).unwrap();
    std::fs::create_dir_all(&db_wt).unwrap();
    write_report(&map_wt, "TICK-REP", "from-hashmap");
    write_report(&db_wt, "TICK-REP", "from-db");

    seed_workflow_in_map(
        &state,
        "TICK-REP",
        &alice_id,
        "ws",
        Some(repo_id.clone()),
        map_wt,
    )
    .await;
    seed_db_row(&state, "TICK-REP", &alice_id, "ws", Some(repo_id), &db_wt).await;

    let (status, body) = fetch_report(&state, "TICK-REP", &alice_cookie).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let content = v["content"].as_str().unwrap();
    assert!(
        content.contains("report-marker:from-db"),
        "route must open the DB row's worktree, got body: {body}"
    );
    assert!(
        !content.contains("from-hashmap"),
        "route must NOT open the HashMap's worktree"
    );

    std::fs::remove_dir_all(&tmp).ok();
}

/// HashMap fallback: when no DB row exists, the route still reads
/// from the in-memory worktree path. Pre-plan-07 workflows must
/// remain functional during the transition.
#[tokio::test]
async fn report_falls_back_to_hashmap_when_no_db_row() {
    let state = test_state_with_db();
    let alice_cookie = register_and_login(&state).await;
    let alice_id = user_id_for(&state, "admin").await;
    let repo_id = seed_repo(&state, "ws", &alice_id).await;

    let tmp = std::env::temp_dir().join(format!("maestro-rep-{}", uuid::Uuid::new_v4()));
    let map_wt = tmp.join("legacy-worktree");
    std::fs::create_dir_all(&map_wt).unwrap();
    write_report(&map_wt, "TICK-LEG", "legacy-fallback");

    seed_workflow_in_map(
        &state,
        "TICK-LEG",
        &alice_id,
        "ws",
        Some(repo_id),
        map_wt,
    )
    .await;
    // NOTE: deliberately no DB row.

    // require_workflow_access uses the DB-first path too — its
    // own fallback to HashMap is what makes this case reachable.
    let (status, body) = fetch_report(&state, "TICK-LEG", &alice_cookie).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let content = v["content"].as_str().unwrap();
    assert!(content.contains("legacy-fallback"));

    std::fs::remove_dir_all(&tmp).ok();
}

/// When the DB row has `worktree_path = NULL`, the route 404s
/// without ever falling back to the HashMap — the DB row is the
/// truth, and "no worktree yet" is a valid state.
#[tokio::test]
async fn report_404s_when_db_row_has_no_worktree() {
    let state = test_state_with_db();
    let alice_cookie = register_and_login(&state).await;
    let alice_id = user_id_for(&state, "admin").await;
    let repo_id = seed_repo(&state, "ws", &alice_id).await;

    let tmp = std::env::temp_dir().join(format!("maestro-rep-{}", uuid::Uuid::new_v4()));
    let map_wt = tmp.join("map-worktree");
    std::fs::create_dir_all(&map_wt).unwrap();
    write_report(&map_wt, "TICK-NOWT", "should-not-be-read");

    // HashMap row WOULD point at a populated worktree, but the DB
    // row says there isn't one — that wins.
    seed_workflow_in_map(
        &state,
        "TICK-NOWT",
        &alice_id,
        "ws",
        Some(repo_id.clone()),
        map_wt,
    )
    .await;

    // Insert DB row with NULL worktree_path.
    let db = state.engine().engine.db().expect("db");
    db.adapter()
        .execute(
            "INSERT INTO work_items (\
                id, ticket_key, workspace_name, user_id, repository_id, private, \
                started_manually, counts_toward_manual_cap, driver_started, \
                jira_available, state_kind, started_at, created_at, updated_at\
             ) VALUES (?, 'TICK-NOWT', 'ws', ?, ?, 0, 0, 0, 0, 0, 'pending', 100, 100, 100)",
            vec![
                DbValue::Text("uuid-TICK-NOWT".into()),
                DbValue::Text(alice_id.clone()),
                DbValue::Text(repo_id),
            ],
        )
        .await
        .expect("insert work_items row");

    let (status, _) = fetch_report(&state, "TICK-NOWT", &alice_cookie).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "DB row's NULL worktree_path is authoritative — must not fall back to HashMap"
    );

    std::fs::remove_dir_all(&tmp).ok();
}
