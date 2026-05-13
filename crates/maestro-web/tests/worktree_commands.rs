// Copyright (C) 2026 Alexandre Obellianne
//
// Integration tests for plan-08 Step 5: admin-gated per-workspace
// `worktree_init_commands` override endpoints.
//
//   GET    /api/admin/worktree-commands
//   GET    /api/admin/worktree-commands/_workspaces
//   GET    /api/admin/worktree-commands/{workspace}
//   PUT    /api/admin/worktree-commands/{workspace}
//   DELETE /api/admin/worktree-commands/{workspace}
//
// We assert:
//   * AC-4: non-admins are rejected with 403 on PUT/DELETE.
//   * AC-3: admin can PUT and read back unchanged.
//   * DELETE returns 204 the first time, 404 the second.
//   * `_workspaces` augments `list_workspaces` with `has_override`.
//   * PUT validates list size (50), per-command length (2000), empty cmds,
//     and NUL bytes.
//   * Top-level GET returns the global default + every override.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

use maestro_web::server::build_router;
use maestro_web::state::AppState;
use maestro_web::test_helpers::{TEST_ORIGIN, register_and_login, test_state_with_db};

async fn create_and_login_regular_user(
    state: &AppState,
    admin_cookie: &str,
    username: &str,
    password: &str,
) -> String {
    let app = build_router(state.clone());
    let body = format!(
        r#"{{"username":"{username}","password":"{password}","role":"user"}}"#
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
        Request::put(format!("/api/admin/worktree-commands/{workspace}"))
            .header("Content-Type", "application/json")
            .header("Origin", TEST_ORIGIN)
            .header("Cookie", cookie)
            .body(Body::from(body_json.to_string()))
            .unwrap(),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn non_admin_gets_403_on_put() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;
    let user_cookie =
        create_and_login_regular_user(&state, &admin_cookie, "bob", "secretpassword123!@#").await;

    let resp = put_commands(&state, &user_cookie, "frontend", r#"{"commands":["echo hi"]}"#).await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn non_admin_gets_403_on_delete() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;
    let user_cookie =
        create_and_login_regular_user(&state, &admin_cookie, "bob", "secretpassword123!@#").await;

    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::builder()
                .method(axum::http::Method::DELETE)
                .uri("/api/admin/worktree-commands/frontend")
                .header("Origin", TEST_ORIGIN)
                .header("Cookie", &user_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn non_admin_gets_403_on_get() {
    // Per task brief: all endpoints admin-gated.
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;
    let user_cookie =
        create_and_login_regular_user(&state, &admin_cookie, "bob", "secretpassword123!@#").await;

    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::get("/api/admin/worktree-commands")
                .header("Cookie", &user_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn admin_can_put_and_get_back_unchanged() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;

    let body = r#"{"commands":["set -e\necho boot","npm ci","cargo build"]}"#;
    let resp = put_commands(&state, &admin_cookie, "frontend", body).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let put_body: Value =
        serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(put_body["workspace_name"], "frontend");
    assert_eq!(
        put_body["commands"],
        serde_json::json!(["set -e\necho boot", "npm ci", "cargo build"])
    );

    // GET reads back identically.
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::get("/api/admin/worktree-commands/frontend")
                .header("Cookie", &admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let get_body: Value =
        serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(get_body["commands"], put_body["commands"]);
    assert_eq!(get_body["workspace_name"], "frontend");
    // updated_by is the admin's user id (non-empty).
    assert!(get_body["updated_by"].is_string());
}

#[tokio::test]
async fn delete_returns_204_then_404() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;

    let resp = put_commands(
        &state,
        &admin_cookie,
        "frontend",
        r#"{"commands":["echo hi"]}"#,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    // First DELETE → 204.
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::builder()
                .method(axum::http::Method::DELETE)
                .uri("/api/admin/worktree-commands/frontend")
                .header("Origin", TEST_ORIGIN)
                .header("Cookie", &admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Second DELETE → 404 (no row to remove).
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::builder()
                .method(axum::http::Method::DELETE)
                .uri("/api/admin/worktree-commands/frontend")
                .header("Origin", TEST_ORIGIN)
                .header("Cookie", &admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn get_one_returns_404_when_no_override() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;

    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::get("/api/admin/worktree-commands/missing")
                .header("Cookie", &admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn workspaces_includes_has_override_flag() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;

    // PUT an override for some workspace name. The `_workspaces` endpoint
    // returns directory entries from /workspaces/* — those won't include this
    // synthetic name in the test environment, but the override row itself is
    // visible to the DB. We assert the response is a JSON array and that the
    // request succeeded (admin-gate test). To assert the flag merges in we
    // would need a workspace on disk; that is environment-dependent and
    // covered by the manual / E2E pass.
    let resp = put_commands(
        &state,
        &admin_cookie,
        "frontend",
        r#"{"commands":["echo hi"]}"#,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::get("/api/admin/worktree-commands/_workspaces")
                .header("Cookie", &admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert!(body.is_array(), "expected JSON array, got {body}");
    // Every entry — if any — has the merged `has_override: bool` key.
    for entry in body.as_array().unwrap() {
        assert!(entry["has_override"].is_boolean(), "entry: {entry}");
        assert!(entry["name"].is_string(), "entry: {entry}");
        assert!(entry["active"].is_boolean(), "entry: {entry}");
    }
}

#[tokio::test]
async fn put_validates_list_size_50() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;

    let cmds: Vec<String> = (0..51).map(|i| format!("echo {i}")).collect();
    let body = serde_json::json!({ "commands": cmds }).to_string();
    let resp = put_commands(&state, &admin_cookie, "frontend", &body).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn put_validates_per_command_2000_chars() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;

    let big = "x".repeat(2001);
    let body = serde_json::json!({ "commands": [big] }).to_string();
    let resp = put_commands(&state, &admin_cookie, "frontend", &body).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn put_rejects_empty_commands() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;

    let resp = put_commands(
        &state,
        &admin_cookie,
        "frontend",
        r#"{"commands":["   "]}"#,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn put_rejects_nul_bytes() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;

    // Use serde_json to inject a NUL byte without breaking the JSON syntax.
    let body = serde_json::json!({ "commands": ["a\u{0000}b"] }).to_string();
    let resp = put_commands(&state, &admin_cookie, "frontend", &body).await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn put_accepts_empty_list_as_override() {
    // Plan AC-9: explicit [] override = present and empty, NOT fallback.
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;

    let resp = put_commands(&state, &admin_cookie, "frontend", r#"{"commands":[]}"#).await;
    assert_eq!(resp.status(), StatusCode::OK);

    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::get("/api/admin/worktree-commands/frontend")
                .header("Cookie", &admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["commands"], serde_json::json!([]));
}

#[tokio::test]
async fn put_rejects_path_traversal_workspace_name() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;

    // axum routing won't match `..` in the path segment cleanly — use a name
    // with a hidden-file prefix, which our validation rejects.
    let resp = put_commands(
        &state,
        &admin_cookie,
        ".hidden",
        r#"{"commands":["echo"]}"#,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn get_top_level_returns_default_plus_overrides() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;

    // Two overrides.
    assert_eq!(
        put_commands(
            &state,
            &admin_cookie,
            "frontend",
            r#"{"commands":["echo one"]}"#,
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        put_commands(
            &state,
            &admin_cookie,
            "backend",
            r#"{"commands":["echo two","echo three"]}"#,
        )
        .await
        .status(),
        StatusCode::OK
    );

    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::get("/api/admin/worktree-commands")
                .header("Cookie", &admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&resp.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert!(body["default"].is_array(), "missing `default`: {body}");
    let overrides = body["overrides"].as_array().expect("missing overrides");
    assert_eq!(overrides.len(), 2);
    // Sorted by workspace_name.
    assert_eq!(overrides[0]["workspace_name"], "backend");
    assert_eq!(
        overrides[0]["commands"],
        serde_json::json!(["echo two", "echo three"])
    );
    assert_eq!(overrides[1]["workspace_name"], "frontend");
    assert_eq!(overrides[1]["commands"], serde_json::json!(["echo one"]));
}

#[tokio::test]
async fn unauthenticated_request_is_401() {
    let state = test_state_with_db();

    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::get("/api/admin/worktree-commands")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
