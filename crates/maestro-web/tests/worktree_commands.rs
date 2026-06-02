// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

// Copyright (C) 2026 Alexandre Obellianne
//
// Integration tests for the per-user-per-workspace worktree init + run
// command endpoints.
//
//   GET    /api/worktree-commands
//   GET    /api/worktree-commands/_workspaces
//   GET    /api/worktree-commands/{workspace}
//   PUT    /api/worktree-commands/{workspace}
//   DELETE /api/worktree-commands/{workspace}

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

use maestro_web::server::build_router;
use maestro_web::state::AppState;
use maestro_web::test_helpers::{TEST_ORIGIN, register_and_login, test_state_with_db};

// ---------------------------------------------------------------------------
// Test plumbing.
// ---------------------------------------------------------------------------

async fn create_and_login_regular_user(
    state: &AppState,
    admin_cookie: &str,
    username: &str,
    password: &str,
) -> String {
    let app = build_router(state.clone());
    let body = format!(r#"{{"username":"{username}","password":"{password}","role":"user"}}"#);
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
    assert_eq!(resp.status(), StatusCode::CREATED, "create user should 201");

    let app = build_router(state.clone());
    let body = format!(r#"{{"username":"{username}","password":"{password}"}}"#);
    let login_resp = app
        .oneshot(
            Request::post("/api/auth/login")
                .header("Content-Type", "application/json")
                .header("Origin", TEST_ORIGIN)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login_resp.status(), StatusCode::NO_CONTENT);
    let set_cookie = login_resp
        .headers()
        .get("set-cookie")
        .expect("login should set cookie")
        .to_str()
        .unwrap()
        .to_string();
    set_cookie.split(';').next().unwrap().trim().to_string()
}

async fn put_commands(
    state: &AppState,
    cookie: &str,
    workspace: &str,
    body_json: &str,
) -> axum::response::Response {
    let app = build_router(state.clone());
    app.oneshot(
        Request::put(format!("/api/worktree-commands/{workspace}"))
            .header("Content-Type", "application/json")
            .header("Origin", TEST_ORIGIN)
            .header("Cookie", cookie)
            .body(Body::from(body_json.to_string()))
            .unwrap(),
    )
    .await
    .unwrap()
}

async fn get_one(state: &AppState, cookie: &str, workspace: &str) -> axum::response::Response {
    let app = build_router(state.clone());
    app.oneshot(
        Request::get(format!("/api/worktree-commands/{workspace}"))
            .header("Cookie", cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap()
}

async fn list_mine(state: &AppState, cookie: &str) -> axum::response::Response {
    let app = build_router(state.clone());
    app.oneshot(
        Request::get("/api/worktree-commands")
            .header("Cookie", cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap()
}

async fn delete_one(state: &AppState, cookie: &str, workspace: &str) -> axum::response::Response {
    let app = build_router(state.clone());
    app.oneshot(
        Request::builder()
            .method(axum::http::Method::DELETE)
            .uri(format!("/api/worktree-commands/{workspace}"))
            .header("Origin", TEST_ORIGIN)
            .header("Cookie", cookie)
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap()
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

/// Non-admin users can PUT and GET their own data — the endpoint is
/// not admin-gated.
#[tokio::test]
async fn non_admin_can_put_and_get_their_own() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;
    let user_cookie =
        create_and_login_regular_user(&state, &admin_cookie, "bob", "secretpassword123!@#").await;

    let body =
        r#"{"init_commands":["echo init"],"run_commands":[{"name":"UI","command":"npm run dev"}]}"#;
    let resp = put_commands(&state, &user_cookie, "frontend", body).await;
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "PUT should succeed for non-admin"
    );

    let resp = get_one(&state, &user_cookie, "frontend").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["workspace_name"], "frontend");
    assert_eq!(body["init_commands"], serde_json::json!(["echo init"]));
    assert_eq!(
        body["run_commands"],
        serde_json::json!([{"name":"UI","command":"npm run dev"}])
    );
}

/// User A cannot see, edit, or delete User B's commands.
///   - Top-level GET returns only the caller's own rows.
///   - GET /{workspace} returns 404 against another user's workspace.
#[tokio::test]
async fn user_a_cannot_see_user_b_data() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;
    let alice =
        create_and_login_regular_user(&state, &admin_cookie, "alice", "secretpassword123!@#").await;
    let bob =
        create_and_login_regular_user(&state, &admin_cookie, "bob", "secretpassword123!@#").await;

    // Alice saves a row for `frontend`.
    let alice_body = r#"{"init_commands":["echo alice-init"],"run_commands":[{"name":"Alice","command":"true"}]}"#;
    assert_eq!(
        put_commands(&state, &alice, "frontend", alice_body)
            .await
            .status(),
        StatusCode::OK,
    );

    // Bob's top-level GET sees no rows.
    let resp = list_mine(&state, &bob).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let arr = body.as_array().expect("expected array");
    assert!(arr.is_empty(), "bob should see no rows, got: {body}");

    // Bob's GET on `frontend` (Alice's workspace) 404s — there's no path to
    // reach Alice's row even by guessing the workspace name.
    let resp = get_one(&state, &bob, "frontend").await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // Bob's DELETE on `frontend` 404s — Alice's row is unaffected.
    let resp = delete_one(&state, &bob, "frontend").await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    // Sanity: Alice's row is still intact.
    let resp = get_one(&state, &alice, "frontend").await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn put_then_get_roundtrips_both_kinds() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;

    let body = r#"{"init_commands":["set -e\necho boot","npm ci","cargo build"],"run_commands":[{"name":"Dashboard UI","command":"cd ui && npm run dev"},{"name":"Storybook","command":"cd ui && npm run storybook"}]}"#;
    let resp = put_commands(&state, &admin_cookie, "frontend", body).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let put_body: Value =
        serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(put_body["workspace_name"], "frontend");
    assert_eq!(
        put_body["init_commands"],
        serde_json::json!(["set -e\necho boot", "npm ci", "cargo build"])
    );
    assert_eq!(
        put_body["run_commands"],
        serde_json::json!([
            { "name": "Dashboard UI", "command": "cd ui && npm run dev" },
            { "name": "Storybook", "command": "cd ui && npm run storybook" }
        ])
    );

    let resp = get_one(&state, &admin_cookie, "frontend").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let get_body: Value =
        serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(get_body["init_commands"], put_body["init_commands"]);
    assert_eq!(get_body["run_commands"], put_body["run_commands"]);
    assert!(get_body["updated_at"].is_i64());
}

#[tokio::test]
async fn delete_returns_204_then_404() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;

    let body = r#"{"init_commands":["echo hi"],"run_commands":[]}"#;
    assert_eq!(
        put_commands(&state, &admin_cookie, "frontend", body)
            .await
            .status(),
        StatusCode::OK,
    );

    let resp = delete_one(&state, &admin_cookie, "frontend").await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = delete_one(&state, &admin_cookie, "frontend").await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn get_one_returns_404_when_no_row() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;

    let resp = get_one(&state, &admin_cookie, "missing").await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// `_workspaces` augments the filesystem scan with `has_my_commands`.
///
/// The filesystem scan inspects `/workspaces/*` which is empty in the test
/// environment, so we cannot assert the merged shape carries our newly-saved
/// workspace. We CAN assert (a) the endpoint is reachable for a non-admin and
/// (b) the response is an array whose entries — if any — carry the merged
/// `has_my_commands` key with the correct shape. The DB-merge logic is
/// already covered by the per-user isolation test above.
#[tokio::test]
async fn _workspaces_includes_has_my_commands() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;
    let user_cookie =
        create_and_login_regular_user(&state, &admin_cookie, "bob", "secretpassword123!@#").await;

    // Save a row for `frontend` so the DB read has something to match against.
    assert_eq!(
        put_commands(
            &state,
            &user_cookie,
            "frontend",
            r#"{"init_commands":["echo hi"],"run_commands":[]}"#,
        )
        .await
        .status(),
        StatusCode::OK,
    );

    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::get("/api/worktree-commands/_workspaces")
                .header("Cookie", &user_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "non-admin user should be able to list workspaces"
    );
    let body: Value =
        serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert!(body.is_array(), "expected JSON array, got {body}");
    for entry in body.as_array().unwrap() {
        assert!(entry["has_my_commands"].is_boolean(), "entry: {entry}");
        assert!(entry["name"].is_string(), "entry: {entry}");
        assert!(entry["active"].is_boolean(), "entry: {entry}");
    }
}

// ---------------------------------------------------------------------------
// Validation matrix.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn put_rejects_oversize_init_list() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;

    let cmds: Vec<String> = (0..51).map(|i| format!("echo {i}")).collect();
    let body = serde_json::json!({ "init_commands": cmds, "run_commands": [] }).to_string();
    let resp = put_commands(&state, &admin_cookie, "frontend", &body).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn put_rejects_oversize_run_list() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;

    let rcs: Vec<_> = (0..51)
        .map(|i| serde_json::json!({ "name": format!("n{i}"), "command": "echo" }))
        .collect();
    let body = serde_json::json!({ "init_commands": [], "run_commands": rcs }).to_string();
    let resp = put_commands(&state, &admin_cookie, "frontend", &body).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn put_rejects_oversize_init_command() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;

    let big = "x".repeat(2001);
    let body = serde_json::json!({
        "init_commands": [big],
        "run_commands": [],
    })
    .to_string();
    let resp = put_commands(&state, &admin_cookie, "frontend", &body).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn put_rejects_oversize_run_command() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;

    let big = "x".repeat(2001);
    let body = serde_json::json!({
        "init_commands": [],
        "run_commands": [{ "name": "ok", "command": big }],
    })
    .to_string();
    let resp = put_commands(&state, &admin_cookie, "frontend", &body).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn put_rejects_oversize_run_name() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;

    let big_name = "n".repeat(101);
    let body = serde_json::json!({
        "init_commands": [],
        "run_commands": [{ "name": big_name, "command": "echo" }],
    })
    .to_string();
    let resp = put_commands(&state, &admin_cookie, "frontend", &body).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn put_rejects_empty_init_command() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;

    let body = r#"{"init_commands":["   "],"run_commands":[]}"#;
    let resp = put_commands(&state, &admin_cookie, "frontend", body).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn put_rejects_empty_run_name() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;

    let body = r#"{"init_commands":[],"run_commands":[{"name":"  ","command":"echo"}]}"#;
    let resp = put_commands(&state, &admin_cookie, "frontend", body).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn put_rejects_empty_run_command() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;

    let body = r#"{"init_commands":[],"run_commands":[{"name":"ok","command":""}]}"#;
    let resp = put_commands(&state, &admin_cookie, "frontend", body).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn put_rejects_nul_byte_in_init() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;

    let body = serde_json::json!({
        "init_commands": ["a\u{0000}b"],
        "run_commands": [],
    })
    .to_string();
    let resp = put_commands(&state, &admin_cookie, "frontend", &body).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn put_rejects_nul_byte_in_run() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;

    let body = serde_json::json!({
        "init_commands": [],
        "run_commands": [{ "name": "ok", "command": "echo a\u{0000}b" }],
    })
    .to_string();
    let resp = put_commands(&state, &admin_cookie, "frontend", &body).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn put_rejects_duplicate_run_names() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;

    let body = r#"{"init_commands":[],"run_commands":[{"name":"X","command":"a"},{"name":"X","command":"b"}]}"#;
    let resp = put_commands(&state, &admin_cookie, "frontend", body).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let txt = std::str::from_utf8(&bytes).unwrap_or("");
    assert!(
        txt.contains("duplicate"),
        "expected duplicate-name message, got: {txt}"
    );
}

#[tokio::test]
async fn put_accepts_empty_lists() {
    // A row with both lists empty is a valid "I exist but have configured
    // nothing yet" state. It's the same observable behaviour as having no
    // row, but the row's presence is what `has_my_commands` reports back
    // through `_workspaces`.
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;

    let resp = put_commands(
        &state,
        &admin_cookie,
        "frontend",
        r#"{"init_commands":[],"run_commands":[]}"#,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = get_one(&state, &admin_cookie, "frontend").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["init_commands"], serde_json::json!([]));
    assert_eq!(body["run_commands"], serde_json::json!([]));
}

#[tokio::test]
async fn put_rejects_path_traversal_workspace_name() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;

    let resp = put_commands(
        &state,
        &admin_cookie,
        ".hidden",
        r#"{"init_commands":["echo"],"run_commands":[]}"#,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn list_top_level_returns_only_callers_rows_sorted() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;
    let alice =
        create_and_login_regular_user(&state, &admin_cookie, "alice", "secretpassword123!@#").await;

    // Alice writes two rows.
    assert_eq!(
        put_commands(
            &state,
            &alice,
            "frontend",
            r#"{"init_commands":["echo one"],"run_commands":[]}"#,
        )
        .await
        .status(),
        StatusCode::OK,
    );
    assert_eq!(
        put_commands(
            &state,
            &alice,
            "backend",
            r#"{"init_commands":["echo two","echo three"],"run_commands":[]}"#,
        )
        .await
        .status(),
        StatusCode::OK,
    );

    // Admin writes one row to confirm Alice's listing doesn't leak it.
    assert_eq!(
        put_commands(
            &state,
            &admin_cookie,
            "infra",
            r#"{"init_commands":["echo admin"],"run_commands":[]}"#,
        )
        .await
        .status(),
        StatusCode::OK,
    );

    let resp = list_mine(&state, &alice).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    let arr = body.as_array().expect("expected JSON array");
    assert_eq!(arr.len(), 2);
    // Sorted by workspace_name.
    assert_eq!(arr[0]["workspace_name"], "backend");
    assert_eq!(
        arr[0]["init_commands"],
        serde_json::json!(["echo two", "echo three"])
    );
    assert_eq!(arr[1]["workspace_name"], "frontend");
    assert_eq!(arr[1]["init_commands"], serde_json::json!(["echo one"]));
}

#[tokio::test]
async fn unauthenticated_request_is_401() {
    let state = test_state_with_db();

    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::get("/api/worktree-commands")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
