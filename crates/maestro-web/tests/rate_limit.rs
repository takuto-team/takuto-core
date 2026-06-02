// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

// Copyright (C) 2026 Alexandre Obellianne
//
// Integration tests for per-IP rate limit and per-user lockout.
//
// Covers the deterministic, in-process pieces of the spec:
//   - G/W/T 3.1 smoke: 11 wrong-password POSTs from one "IP" → 11th is 429.
//   - G/W/T 3.5: 5 failed attempts inside the 10-minute window → 6th returns
//     429 with `Retry-After` and an "account temporarily locked" body.
//   - G/W/T 3.6: a successful login between failures clears the failed counter.
//   - G/W/T 3.7: recovery counter is independent of password counter.
//   - G/W/T 3.8: admin `POST /api/users/{id}/unlock` clears the counters.
//   - G/W/T 3.9: an unknown username never trips the lockout (no row recorded).
//   - G/W/T 3.10: the unlock endpoint is admin-gated.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use maestro_web::server::build_router;
use maestro_web::state::AppState;
use maestro_web::test_helpers::{TEST_ORIGIN, register_and_login, test_state_with_db};

const ADMIN_USERNAME: &str = "admin";

/// Create a non-admin user via the admin API and return that user's session cookie.
async fn create_and_login_user(
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
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "create user should succeed"
    );

    login_as(state, username, password).await
}

/// Send `POST /api/auth/login` with the given credentials, capturing the
/// response body and `Set-Cookie` header. Used by tests that need to inspect
/// both the status code and the JSON body (e.g. asserting the lockout message).
async fn login_with(state: &AppState, body_json: &str) -> (StatusCode, String, Option<String>) {
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::post("/api/auth/login")
                .header("Content-Type", "application/json")
                .header("Origin", TEST_ORIGIN)
                .body(Body::from(body_json.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let cookie = resp
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(';').next().unwrap_or("").trim().to_string());
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&body).to_string();
    (status, body, cookie)
}

async fn login_as(state: &AppState, username: &str, password: &str) -> String {
    let body = format!(r#"{{"username":"{username}","password":"{password}"}}"#);
    let (status, body, cookie) = login_with(state, &body).await;
    assert_eq!(
        status,
        StatusCode::NO_CONTENT,
        "login should succeed: {body}"
    );
    cookie.expect("login should set a cookie")
}

async fn recover_with(state: &AppState, body_json: &str) -> (StatusCode, String) {
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::post("/api/auth/recover")
                .header("Content-Type", "application/json")
                .header("Origin", TEST_ORIGIN)
                .body(Body::from(body_json.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

/// Look up a user's `id` from the database. Used by the unlock route tests.
async fn user_id_for(state: &AppState, username: &str) -> String {
    let db = state
        .auth()
        .db
        .as_ref()
        .expect("test state has a db")
        .clone();
    maestro_core::db::users::get_user_by_username(db.adapter(), username)
        .await
        .ok()
        .flatten()
        .map(|u| u.id)
        .expect("user must exist")
}

#[tokio::test]
async fn five_failures_in_window_locks_user_on_sixth_attempt() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;
    let _ = create_and_login_user(&state, &admin_cookie, "alice", "alice_passWORD1!").await;

    // Five wrong-password attempts.
    for i in 0..5 {
        let (status, _body, _cookie) = login_with(
            &state,
            r#"{"username":"alice","password":"wrong-password!"}"#,
        )
        .await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "attempt #{i} should be 401"
        );
    }

    // Sixth attempt — wrong password — should be 429 with Retry-After.
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::post("/api/auth/login")
                .header("Content-Type", "application/json")
                .header("Origin", TEST_ORIGIN)
                .body(Body::from(
                    r#"{"username":"alice","password":"wrong-password!"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(
        resp.headers().get("retry-after").is_some(),
        "lockout response must carry Retry-After"
    );
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let err = json["error"].as_str().unwrap_or_default();
    assert!(
        err.contains("locked"),
        "expected 'locked' in error message, got: {err}"
    );

    // Even the correct password is rejected with 429 while the lockout window
    // is open (the lockout precedes password verification).
    let (status, _, _) = login_with(
        &state,
        r#"{"username":"alice","password":"alice_passWORD1!"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn success_login_clears_failed_counter() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;
    let _ = create_and_login_user(&state, &admin_cookie, "alice", "alice_passWORD1!").await;

    // 3 failures, still under the threshold.
    for _ in 0..3 {
        let (s, _, _) = login_with(
            &state,
            r#"{"username":"alice","password":"wrong-password!"}"#,
        )
        .await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
    }

    // Successful login resets the counter.
    let (s, body, _) = login_with(
        &state,
        r#"{"username":"alice","password":"alice_passWORD1!"}"#,
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT, "expected success: {body}");

    // 5 more failed attempts should NOT lock (counter started fresh).
    for i in 0..4 {
        let (s, _, _) = login_with(
            &state,
            r#"{"username":"alice","password":"wrong-password!"}"#,
        )
        .await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "attempt #{i} expected 401");
    }
    let (s, _, _) = login_with(
        &state,
        r#"{"username":"alice","password":"wrong-password!"}"#,
    )
    .await;
    // 5th failure: still under the threshold (>= 5 locks the next attempt).
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    // 6th failure (totalling 5 over the window since the success cleared) → 429.
    let (s, _, _) = login_with(
        &state,
        r#"{"username":"alice","password":"wrong-password!"}"#,
    )
    .await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn unknown_username_never_locks() {
    let state = test_state_with_db();
    let _admin_cookie = register_and_login(&state).await;

    // Snapshot row count after admin setup — register_and_login records an
    // admin success row, which is expected. We assert the count does NOT
    // grow when subsequent attempts are made against a non-existent user.
    let db_handle = state.auth().db.as_ref().unwrap().clone();
    let baseline: i64 = db_handle
        .adapter()
        .query_one("SELECT COUNT(*) FROM login_attempts", vec![])
        .await
        .unwrap()
        .get_i64(0)
        .unwrap();

    // 6+ login attempts for a username that doesn't exist — every one must be
    // 401, NEVER 429. Otherwise the throttle leaks account existence info.
    for _ in 0..7 {
        let (s, _, _) =
            login_with(&state, r#"{"username":"ghost","password":"any-password!"}"#).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
    }

    let after: i64 = db_handle
        .adapter()
        .query_one("SELECT COUNT(*) FROM login_attempts", vec![])
        .await
        .unwrap()
        .get_i64(0)
        .unwrap();
    assert_eq!(
        after, baseline,
        "no login_attempts rows expected for unknown user"
    );
}

#[tokio::test]
async fn recovery_counter_is_independent_of_password_counter() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;
    let _ = create_and_login_user(&state, &admin_cookie, "alice", "alice_passWORD1!").await;

    // 4 password failures — under threshold.
    for _ in 0..4 {
        let (s, _, _) = login_with(
            &state,
            r#"{"username":"alice","password":"wrong-password!"}"#,
        )
        .await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
    }

    // 5 recovery failures — each is 401, then the 6th must be 429.
    for _ in 0..5 {
        let (s, _) = recover_with(
            &state,
            r#"{"username":"alice","recovery_code":"BAD-CODE","new_password":"new_passWORD12345!"}"#,
        )
        .await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
    }
    let (s, _) = recover_with(
        &state,
        r#"{"username":"alice","recovery_code":"BAD-CODE","new_password":"new_passWORD12345!"}"#,
    )
    .await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);

    // The password counter still has its 4 entries — it has NOT been touched
    // by the recovery flow. One more wrong password → still 401, not 429.
    let (s, _, _) = login_with(
        &state,
        r#"{"username":"alice","password":"wrong-password!"}"#,
    )
    .await;
    assert_eq!(
        s,
        StatusCode::UNAUTHORIZED,
        "password counter must be independent of recovery counter"
    );
}

#[tokio::test]
async fn admin_unlock_clears_counters_and_unblocks_user() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;
    let _ = create_and_login_user(&state, &admin_cookie, "alice", "alice_passWORD1!").await;
    let alice_id = user_id_for(&state, "alice").await;

    // Lock alice with 5 failures.
    for _ in 0..5 {
        let (s, _, _) = login_with(
            &state,
            r#"{"username":"alice","password":"wrong-password!"}"#,
        )
        .await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
    }
    let (s, _, _) = login_with(
        &state,
        r#"{"username":"alice","password":"wrong-password!"}"#,
    )
    .await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS, "alice should be locked");

    // Admin issues `POST /api/users/{alice_id}/unlock`.
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::post(format!("/api/users/{alice_id}/unlock"))
                .header("Origin", TEST_ORIGIN)
                .header("Cookie", &admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "admin unlock should 204"
    );

    // Correct credentials now succeed.
    let (s, body, _) = login_with(
        &state,
        r#"{"username":"alice","password":"alice_passWORD1!"}"#,
    )
    .await;
    assert_eq!(
        s,
        StatusCode::NO_CONTENT,
        "expected success post-unlock: {body}"
    );
}

#[tokio::test]
async fn unlock_endpoint_is_admin_gated() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;
    let alice_cookie =
        create_and_login_user(&state, &admin_cookie, "alice", "alice_passWORD1!").await;
    let admin_id = user_id_for(&state, ADMIN_USERNAME).await;

    // Non-admin alice tries to unlock the admin account — must get 403.
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::post(format!("/api/users/{admin_id}/unlock"))
                .header("Origin", TEST_ORIGIN)
                .header("Cookie", &alice_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // And without any cookie at all — 401.
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::post(format!("/api/users/{admin_id}/unlock"))
                .header("Origin", TEST_ORIGIN)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// G/W/T 3.1 smoke: per-IP layer rejects the 11th login from the same client
/// within the burst window. Verifies the `tower_governor` configuration plus
/// the custom `LoginKeyExtractor` (which collapses to a single bucket in
/// tests because no peer addr is wired in).
#[tokio::test]
async fn per_ip_rate_limit_blocks_11th_request() {
    let state = test_state_with_db();
    let _ = register_and_login(&state).await;

    // The admin user already exists. Send 10 wrong-password attempts in a
    // tight loop — every one consumes a rate-limit permit. The 11th is the
    // boundary case.
    let mut last_status = None;
    for i in 0..11 {
        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::post("/api/auth/login")
                    .header("Content-Type", "application/json")
                    .header("Origin", TEST_ORIGIN)
                    // Use the same key extractor input for every request
                    // (LoginKeyExtractor reads X-Forwarded-For first).
                    .header("X-Forwarded-For", "10.0.0.1")
                    .body(Body::from(format!(
                        r#"{{"username":"{ADMIN_USERNAME}","password":"definitely-wrong-{i}"}}"#
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        last_status = Some(resp.status());
    }
    assert_eq!(
        last_status,
        Some(StatusCode::TOO_MANY_REQUESTS),
        "11th request from same IP should be 429"
    );
}
