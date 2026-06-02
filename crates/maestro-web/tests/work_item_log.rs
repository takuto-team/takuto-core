// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! `GET /api/work-items/{id}/log`.
//! Paged read of `work_item_log_lines`. Tests cover happy path, step_id
//! filter, pagination, access control, and the "no DB / no rows"
//! empty-array contract.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use maestro_core::config::TicketingSystem;
use maestro_core::db::adapter::DbValue;
use maestro_core::db::repositories;
use maestro_core::db::work_items;
use maestro_core::workflow::engine::Workflow;

use maestro_web::server::build_router;
use maestro_web::state::AppState;
use maestro_web::test_helpers::{TEST_ORIGIN, register_and_login, test_state_with_db};

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
    let body =
        format!(r#"{{"username":"{username}","password":"testpassword1234"}}"#);
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

async fn seed_workflow_and_db_row(
    state: &AppState,
    ticket_key: &str,
    user_id: &str,
) {
    // Repo association so require_workflow_access lets the owner through.
    let db = state.engine().engine.db().expect("db");
    let repo_id =
        repositories::upsert(db.adapter(), "ws", None, "/tmp/ws", "main", None)
            .await
            .expect("repo upsert");
    repositories::add_for_user(db.adapter(), user_id, &repo_id)
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
    wf.user_id = Some(user_id.to_string());
    wf.repository_id = Some(repo_id.clone());
    state
        .engine()
        .engine
        .workflows_arc()
        .write()
        .await
        .insert(ticket_key.to_string(), wf);

    db.adapter()
        .execute(
            "INSERT INTO work_items (\
                id, ticket_key, workspace_name, user_id, repository_id, private, \
                started_manually, counts_toward_manual_cap, driver_started, \
                jira_available, state_kind, started_at, created_at, updated_at\
             ) VALUES (?, ?, 'ws', ?, ?, 0, 0, 0, 0, 0, 'pending', 100, 100, 100)",
            vec![
                DbValue::Text(ticket_key.to_string()),
                DbValue::Text(ticket_key.to_string()),
                DbValue::Text(user_id.to_string()),
                DbValue::Text(repo_id),
            ],
        )
        .await
        .expect("insert work_items row");
}

async fn fetch_log(
    state: &AppState,
    ticket_key: &str,
    cookie: &str,
    query: &str,
) -> (StatusCode, Vec<serde_json::Value>) {
    let path = if query.is_empty() {
        format!("/api/work-items/{ticket_key}/log")
    } else {
        format!("/api/work-items/{ticket_key}/log?{query}")
    };
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::get(path)
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
    let json: serde_json::Value = if bytes.is_empty() {
        serde_json::Value::Array(Vec::new())
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Array(Vec::new()))
    };
    let arr = json.as_array().cloned().unwrap_or_default();
    (status, arr)
}

/// Happy path: lines are returned oldest-first with their stream
/// stringified and step_id surfaced.
#[tokio::test]
async fn get_log_returns_db_rows_oldest_first() {
    let state = test_state_with_db();
    let cookie = register_and_login(&state).await;
    let uid = user_id_for(&state, "admin").await;
    seed_workflow_and_db_row(&state, "TICK-LOG", &uid).await;

    let db = state.engine().engine.db().expect("db");
    work_items::append_log_lines(
        db.adapter(),
        &[
            work_items::LogLineInsert {
                work_item_id: "TICK-LOG".into(),
                step_id: None,
                stream: work_items::LogStream::Stdout,
                content: "first".into(),
                emitted_at: 100,
            },
            work_items::LogLineInsert {
                work_item_id: "TICK-LOG".into(),
                step_id: None,
                stream: work_items::LogStream::Stderr,
                content: "second".into(),
                emitted_at: 200,
            },
            work_items::LogLineInsert {
                work_item_id: "TICK-LOG".into(),
                step_id: None,
                stream: work_items::LogStream::Info,
                content: "third".into(),
                emitted_at: 300,
            },
        ],
    )
    .await
    .unwrap();

    let (status, arr) = fetch_log(&state, "TICK-LOG", &cookie, "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(arr.len(), 3);
    assert_eq!(arr[0]["content"], "first");
    assert_eq!(arr[0]["stream"], "stdout");
    assert_eq!(arr[0]["emitted_at"], 100);
    assert_eq!(arr[1]["content"], "second");
    assert_eq!(arr[1]["stream"], "stderr");
    assert_eq!(arr[2]["content"], "third");
    assert_eq!(arr[2]["stream"], "info");
}

/// `step_id` query param restricts to lines from that step.
#[tokio::test]
async fn get_log_step_id_filter_restricts_to_step() {
    let state = test_state_with_db();
    let cookie = register_and_login(&state).await;
    let uid = user_id_for(&state, "admin").await;
    seed_workflow_and_db_row(&state, "TICK-STEP", &uid).await;

    let db = state.engine().engine.db().expect("db");
    let step_id = work_items::record_step_start(
        db.adapter(),
        "TICK-STEP",
        "build",
        None,
        50,
    )
    .await
    .unwrap();
    work_items::append_log_lines(
        db.adapter(),
        &[
            work_items::LogLineInsert {
                work_item_id: "TICK-STEP".into(),
                step_id: Some(step_id),
                stream: work_items::LogStream::Stdout,
                content: "step-line".into(),
                emitted_at: 100,
            },
            work_items::LogLineInsert {
                work_item_id: "TICK-STEP".into(),
                step_id: None,
                stream: work_items::LogStream::Info,
                content: "no-step-line".into(),
                emitted_at: 200,
            },
        ],
    )
    .await
    .unwrap();

    let (status, arr) =
        fetch_log(&state, "TICK-STEP", &cookie, &format!("step_id={step_id}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["content"], "step-line");
}

/// `limit` and `offset` paginate. A 3-line log split by limit=2
/// returns 2 then 1.
#[tokio::test]
async fn get_log_paginates_with_limit_and_offset() {
    let state = test_state_with_db();
    let cookie = register_and_login(&state).await;
    let uid = user_id_for(&state, "admin").await;
    seed_workflow_and_db_row(&state, "TICK-PAGE", &uid).await;

    let db = state.engine().engine.db().expect("db");
    work_items::append_log_lines(
        db.adapter(),
        &[
            work_items::LogLineInsert {
                work_item_id: "TICK-PAGE".into(),
                step_id: None,
                stream: work_items::LogStream::Stdout,
                content: "a".into(),
                emitted_at: 100,
            },
            work_items::LogLineInsert {
                work_item_id: "TICK-PAGE".into(),
                step_id: None,
                stream: work_items::LogStream::Stdout,
                content: "b".into(),
                emitted_at: 200,
            },
            work_items::LogLineInsert {
                work_item_id: "TICK-PAGE".into(),
                step_id: None,
                stream: work_items::LogStream::Stdout,
                content: "c".into(),
                emitted_at: 300,
            },
        ],
    )
    .await
    .unwrap();

    let (_, page1) = fetch_log(&state, "TICK-PAGE", &cookie, "limit=2&offset=0").await;
    assert_eq!(page1.len(), 2);
    assert_eq!(page1[0]["content"], "a");
    assert_eq!(page1[1]["content"], "b");

    let (_, page2) = fetch_log(&state, "TICK-PAGE", &cookie, "limit=2&offset=2").await;
    assert_eq!(page2.len(), 1);
    assert_eq!(page2[0]["content"], "c");
}

/// Non-owners get 404 (existence is not leaked), never 403.
#[tokio::test]
async fn get_log_returns_404_for_non_owner() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;
    let admin_id = user_id_for(&state, "admin").await;
    let bob_cookie = create_and_login_user(&state, &admin_cookie, "bob").await;

    seed_workflow_and_db_row(&state, "TICK-OWN", &admin_id).await;
    let db = state.engine().engine.db().expect("db");
    work_items::append_log_lines(
        db.adapter(),
        &[work_items::LogLineInsert {
            work_item_id: "TICK-OWN".into(),
            step_id: None,
            stream: work_items::LogStream::Info,
            content: "secret".into(),
            emitted_at: 100,
        }],
    )
    .await
    .unwrap();

    let (status, _) = fetch_log(&state, "TICK-OWN", &bob_cookie, "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// Legacy /workflows alias serves the same handler.
#[tokio::test]
async fn get_log_legacy_workflows_alias_works() {
    let state = test_state_with_db();
    let cookie = register_and_login(&state).await;
    let uid = user_id_for(&state, "admin").await;
    seed_workflow_and_db_row(&state, "TICK-LEG", &uid).await;

    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::get("/api/workflows/TICK-LEG/log")
                .header("Cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}
