// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Plan-07 slice 9 — `GET /api/work-items/{id}/steps`.
//!
//! Verifies the new pure-read endpoint at the route layer:
//! seeds work_item_steps rows directly, hits the route through
//! the full router, and asserts both the success shape and the
//! `require_workflow_access` policy (404 for non-owners).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use maestro_core::config::TicketingSystem;
use maestro_core::db::adapter::DbValue;
use maestro_core::db::work_items;
use maestro_core::workflow::engine::Workflow;

use maestro_web::server::build_router;
use maestro_web::state::AppState;
use maestro_web::test_helpers::{TEST_ORIGIN, register_and_login, test_state_with_db};

/// Register a non-admin user via the admin endpoint and log them in.
async fn create_and_login_user(
    state: &AppState,
    admin_cookie: &str,
    username: &str,
) -> String {
    let app = build_router(state.clone());
    let body = format!(
        r#"{{"username":"{username}","password":"testpassword1234","role":"user"}}"#,
    );
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
    let body = format!(
        r#"{{"username":"{username}","password":"testpassword1234"}}"#
    );
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

/// Insert a workflow into the engine map and seed a matching
/// `work_items` row in the DB so the FK on `work_item_steps`
/// resolves.
async fn seed_workflow_and_db_row(
    state: &AppState,
    wf_id: &str,
    ticket_key: &str,
    user_id: &str,
) {
    // Plan-10: require_workflow_access checks that the workflow's
    // repository (or workspace_name fallback) is one the caller has
    // added. Seed a repo named "ws" and associate it with this user.
    let db = state.engine().engine.db().expect("db");
    let repo_id =
        maestro_core::db::repositories::upsert(db.adapter(), "ws", None, "/tmp/ws", "main", None)
            .await
            .expect("repo upsert");
    maestro_core::db::repositories::add_for_user(db.adapter(), user_id, &repo_id)
        .await
        .expect("add_for_user");

    let mut wf = Workflow::new(
        ticket_key.to_string(),
        "Summary".into(),
        false,
        false,
        TicketingSystem::None,
        None,
        "ws".into(),
    );
    wf.id = wf_id.to_string();
    wf.user_id = Some(user_id.to_string());
    wf.repository_id = Some(repo_id);
    state
        .engine()
        .engine
        .workflows_arc()
        .write()
        .await
        .insert(ticket_key.to_string(), wf);

    // Match the engine's in-memory keying convention. The route
    // takes the path id as the work_item key; `require_workflow_access`
    // looks the workflow up under that key.
    db.adapter()
        .execute(
            "INSERT INTO work_items (\
                id, ticket_key, workspace_name, user_id, private, \
                started_manually, counts_toward_manual_cap, driver_started, \
                jira_available, state_kind, started_at, created_at, updated_at\
             ) VALUES (?, ?, 'ws', ?, 0, 0, 0, 0, 0, 'pending', 100, 100, 100)",
            vec![
                DbValue::Text(wf_id.to_string()),
                DbValue::Text(ticket_key.to_string()),
                DbValue::Text(user_id.to_string()),
            ],
        )
        .await
        .expect("seed work_items");
}

#[tokio::test]
async fn get_steps_returns_db_rows_ascending_by_sequence() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;
    let admin_id = user_id_for(&state, "admin").await;

    // The id used to look up the workflow in the engine map is the
    // path segment. Engine's `workflows` map is keyed by ticket_key
    // (see `require_workflow_access`), so use ticket_key as the route
    // path AND as the work_item id for shadow-write FK.
    seed_workflow_and_db_row(&state, "TICK-1", "TICK-1", &admin_id).await;

    // Seed two step rows directly so the test doesn't depend on the
    // engine actually running a workflow.
    let db = state.engine().engine.db().expect("db");
    let id1 = work_items::record_step_start(
        db.adapter(),
        "TICK-1",
        "build (run 1/1)",
        Some("ship.toml"),
        200,
    )
    .await
    .expect("start 1");
    work_items::record_step_end(
        db.adapter(),
        id1,
        work_items::StepStatus::Success,
        Some(0),
        None,
        300,
    )
    .await
    .expect("end 1");
    let _id2 = work_items::record_step_start(
        db.adapter(),
        "TICK-1",
        "test (run 1/1)",
        Some("ship.toml"),
        400,
    )
    .await
    .expect("start 2");

    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::get("/api/work-items/TICK-1/steps")
                .header("Cookie", &admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let arr = v.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    // Ascending by sequence: build (0) before test (1).
    assert_eq!(arr[0]["name"], "build (run 1/1)");
    assert_eq!(arr[0]["sequence"], 0);
    assert_eq!(arr[0]["status"], "success");
    assert_eq!(arr[0]["exit_code"], 0);
    assert_eq!(arr[0]["ended_at"], 300);
    assert_eq!(arr[0]["definition_filename"], "ship.toml");
    assert_eq!(arr[1]["name"], "test (run 1/1)");
    assert_eq!(arr[1]["sequence"], 1);
    assert_eq!(arr[1]["status"], "running");
    assert!(arr[1]["ended_at"].is_null());
}

#[tokio::test]
async fn get_steps_legacy_workflows_alias_works() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;
    let admin_id = user_id_for(&state, "admin").await;
    seed_workflow_and_db_row(&state, "TICK-LEG", "TICK-LEG", &admin_id).await;

    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::get("/api/workflows/TICK-LEG/steps")
                .header("Cookie", &admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "legacy /workflows alias must serve the same handler"
    );
}

#[tokio::test]
async fn get_steps_returns_404_for_non_owner() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;
    let admin_id = user_id_for(&state, "admin").await;
    let bob_cookie = create_and_login_user(&state, &admin_cookie, "bob").await;

    // Workflow owned by admin; bob must not see it.
    seed_workflow_and_db_row(&state, "TICK-S", "TICK-S", &admin_id).await;
    let db = state.engine().engine.db().expect("db");
    work_items::record_step_start(db.adapter(), "TICK-S", "build", None, 200)
        .await
        .expect("step");

    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::get("/api/work-items/TICK-S/steps")
                .header("Cookie", &bob_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "non-owner must get 404, NOT 403 (AC-2 convention)"
    );
}

#[tokio::test]
async fn get_steps_returns_empty_for_workflow_with_no_steps() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;
    let admin_id = user_id_for(&state, "admin").await;
    seed_workflow_and_db_row(&state, "TICK-EMPTY", "TICK-EMPTY", &admin_id).await;

    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::get("/api/work-items/TICK-EMPTY/steps")
                .header("Cookie", &admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(
        std::str::from_utf8(&bytes).unwrap(),
        "[]",
        "empty array, not 404"
    );
}
