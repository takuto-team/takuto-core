// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

// Copyright (C) 2026 Alexandre Obellianne
//
// Integration tests for plan-02 AC-5 — session rotation, sliding-extend, and
// absolute TTL.
//
// Time-dependent cases drive a test-only clock seam in `maestro_web::auth`
// (`set_test_now_unix` / `clear_test_now_unix`) so we never sleep — the
// validation logic always reads `now_unix()` which honours the override.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use maestro_core::config::Config;
use maestro_web::auth::{
    SESSION_ABSOLUTE_TTL_SECS, SESSION_EXTEND_THRESHOLD_SECS, SESSION_IDLE_TTL_SECS,
    clear_test_now_unix, set_test_now_unix,
};
use maestro_web::server::build_router;
use maestro_web::state::AppState;
use maestro_web::test_helpers::{TEST_ORIGIN, register_and_login, test_state_with_db};

/// All time-driving tests share a global clock seam, so they must NOT run
/// in parallel. Cargo test threads each test, so we serialise via a mutex.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Pick a fixed t0 for time-driving tests. Wall-clock value far enough into
/// the past that adding `SESSION_IDLE_TTL_SECS` etc. doesn't overflow, and
/// strictly positive so the `created_at_unix > 0` gate fires.
fn anchor_t0() -> i64 {
    // 2026-01-01T00:00:00Z (UTC) — stable, large, > 30d into Unix epoch.
    1_767_225_600
}

async fn login_with_username(state: &AppState, username: &str, password: &str) -> String {
    let app = build_router(state.clone());
    let body = format!(r#"{{"username":"{username}","password":"{password}"}}"#);
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
    resp.headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .trim()
        .to_string()
}

async fn create_user(state: &AppState, admin_cookie: &str, username: &str, password: &str) {
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
    assert_eq!(resp.status(), StatusCode::CREATED);
}

async fn me_status(state: &AppState, cookie: &str) -> StatusCode {
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::get("/api/auth/me")
                .header("Cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    resp.status()
}

async fn user_id_for(state: &AppState, username: &str) -> String {
    let db = state.auth.db.as_ref().unwrap().clone();
    let username = username.to_string();
    tokio::task::spawn_blocking(move || {
        let conn = db.conn().blocking_lock();
        maestro_core::db::users::get_user_by_username(&conn, &username)
            .ok()
            .flatten()
            .unwrap()
            .id
    })
    .await
    .unwrap()
}

/// G/W/T 5.1: a second login for the same user invalidates the first session
/// when `kick_other_sessions_on_login = true` (the default).
#[tokio::test]
async fn second_login_kicks_prior_session_by_default() {
    let _g = SERIAL.lock().await;
    clear_test_now_unix();

    let state = test_state_with_db();
    let _admin = register_and_login(&state).await;
    create_user(&state, &_admin, "alice", "alice_passWORD1!").await;

    let s1 = login_with_username(&state, "alice", "alice_passWORD1!").await;
    assert_eq!(me_status(&state, &s1).await, StatusCode::OK);

    let s2 = login_with_username(&state, "alice", "alice_passWORD1!").await;
    assert_ne!(s1, s2, "second login should issue a different cookie");
    assert_eq!(
        me_status(&state, &s1).await,
        StatusCode::UNAUTHORIZED,
        "old cookie must be invalidated"
    );
    assert_eq!(me_status(&state, &s2).await, StatusCode::OK);
}

/// G/W/T 5.2: with `kick_other_sessions_on_login = false`, both cookies stay
/// valid after a second login.
#[tokio::test]
async fn second_login_keeps_prior_session_when_flag_disabled() {
    let _g = SERIAL.lock().await;
    clear_test_now_unix();

    let state = test_state_with_db();
    {
        let mut cfg = state.config.config.write().await;
        cfg.web.kick_other_sessions_on_login = false;
    }
    let _admin = register_and_login(&state).await;
    create_user(&state, &_admin, "alice", "alice_passWORD1!").await;

    let s1 = login_with_username(&state, "alice", "alice_passWORD1!").await;
    let s2 = login_with_username(&state, "alice", "alice_passWORD1!").await;
    assert_eq!(me_status(&state, &s1).await, StatusCode::OK);
    assert_eq!(me_status(&state, &s2).await, StatusCode::OK);
}

/// G/W/T 5.3: changing a user's role via `PATCH /api/users/{id}` deletes all
/// sessions for that user — the new role takes effect immediately, the old
/// session no longer authenticates.
#[tokio::test]
async fn role_change_invalidates_target_user_sessions() {
    let _g = SERIAL.lock().await;
    clear_test_now_unix();

    let state = test_state_with_db();
    let admin = register_and_login(&state).await;
    create_user(&state, &admin, "alice", "alice_passWORD1!").await;
    let alice_id = user_id_for(&state, "alice").await;
    let alice_cookie = login_with_username(&state, "alice", "alice_passWORD1!").await;
    assert_eq!(me_status(&state, &alice_cookie).await, StatusCode::OK);

    // Admin promotes alice to admin.
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(format!("/api/users/{alice_id}"))
                .header("Content-Type", "application/json")
                .header("Origin", TEST_ORIGIN)
                .header("Cookie", &admin)
                .body(Body::from(r#"{"role":"admin"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "role change should succeed");

    // alice's old cookie is dead — she has to log in again.
    assert_eq!(
        me_status(&state, &alice_cookie).await,
        StatusCode::UNAUTHORIZED,
        "old cookie must be killed after role change"
    );
    let new_cookie = login_with_username(&state, "alice", "alice_passWORD1!").await;
    assert_eq!(me_status(&state, &new_cookie).await, StatusCode::OK);
}

/// G/W/T 5.4 + 5.5: an authenticated request beyond the 5-minute extend
/// threshold updates `last_seen_at` and slides `expires_at`; a request inside
/// the threshold does NOT issue an `UPDATE`.
#[tokio::test]
async fn sliding_extend_only_when_threshold_crossed() {
    let _g = SERIAL.lock().await;
    let t0 = anchor_t0();
    set_test_now_unix(t0);

    let state = test_state_with_db();
    let admin = register_and_login(&state).await;
    create_user(&state, &admin, "alice", "alice_passWORD1!").await;

    let alice_cookie = login_with_username(&state, "alice", "alice_passWORD1!").await;
    let alice_id = user_id_for(&state, "alice").await;

    // Snapshot baseline last_seen_at right after login.
    let db = state.auth.db.as_ref().unwrap().clone();
    let alice_uid = alice_id.clone();
    let initial_last_seen: i64 = tokio::task::spawn_blocking({
        let db = db.clone();
        move || {
            let conn = db.conn().blocking_lock();
            conn.query_row(
                "SELECT last_seen_at FROM sessions WHERE user_id = ?1",
                rusqlite::params![alice_uid],
                |r| r.get(0),
            )
            .unwrap()
        }
    })
    .await
    .unwrap();
    assert_eq!(initial_last_seen, t0);

    // Bump the clock by 2 minutes — below the 5-minute threshold.
    set_test_now_unix(t0 + 120);
    assert_eq!(me_status(&state, &alice_cookie).await, StatusCode::OK);

    let alice_uid = alice_id.clone();
    let after_short: i64 = tokio::task::spawn_blocking({
        let db = db.clone();
        move || {
            let conn = db.conn().blocking_lock();
            conn.query_row(
                "SELECT last_seen_at FROM sessions WHERE user_id = ?1",
                rusqlite::params![alice_uid],
                |r| r.get(0),
            )
            .unwrap()
        }
    })
    .await
    .unwrap();
    assert_eq!(
        after_short, initial_last_seen,
        "no UPDATE should happen inside the 5-minute threshold"
    );

    // Bump well past the threshold (6 minutes).
    set_test_now_unix(t0 + (SESSION_EXTEND_THRESHOLD_SECS as i64) + 60);
    assert_eq!(me_status(&state, &alice_cookie).await, StatusCode::OK);

    let alice_uid = alice_id.clone();
    let after_long: (i64, String) = tokio::task::spawn_blocking({
        let db = db.clone();
        move || {
            let conn = db.conn().blocking_lock();
            conn.query_row(
                "SELECT last_seen_at, expires_at FROM sessions WHERE user_id = ?1",
                rusqlite::params![alice_uid],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
            )
            .unwrap()
        }
    })
    .await
    .unwrap();
    assert!(
        after_long.0 > initial_last_seen,
        "last_seen_at should be bumped past the threshold (was {initial_last_seen}, now {})",
        after_long.0
    );
    // The new expires_at should also reflect the slide: roughly t0 + 6min + idle_ttl.
    let expected_exp = t0 + (SESSION_EXTEND_THRESHOLD_SECS as i64) + 60 + SESSION_IDLE_TTL_SECS as i64;
    let parsed = chrono::DateTime::parse_from_rfc3339(&after_long.1)
        .unwrap()
        .timestamp();
    assert!(
        (parsed - expected_exp).abs() <= 2,
        "expires_at = {parsed}, expected ≈ {expected_exp}"
    );

    clear_test_now_unix();
}

/// G/W/T 5.7: absolute TTL rejects a session past 30 days even with continuous
/// activity. We pre-seed `created_at_unix` to far in the past and verify the
/// auth middleware rejects + deletes the row.
#[tokio::test]
async fn absolute_ttl_rejects_and_deletes_old_session() {
    let _g = SERIAL.lock().await;
    let t0 = anchor_t0();
    set_test_now_unix(t0);

    let state = test_state_with_db();
    let admin = register_and_login(&state).await;
    create_user(&state, &admin, "alice", "alice_passWORD1!").await;
    let alice_cookie = login_with_username(&state, "alice", "alice_passWORD1!").await;

    // Force the row to look "ancient": created 31 days ago, last_seen now.
    let db = state.auth.db.as_ref().unwrap().clone();
    let ancient = t0 - (SESSION_ABSOLUTE_TTL_SECS as i64) - 60;
    tokio::task::spawn_blocking({
        let db = db.clone();
        move || {
            let conn = db.conn().blocking_lock();
            conn.execute(
                "UPDATE sessions SET created_at_unix = ?1 WHERE user_id = (SELECT id FROM users WHERE username='alice')",
                rusqlite::params![ancient],
            )
            .unwrap();
        }
    })
    .await
    .unwrap();

    // The middleware should reject and delete the row.
    assert_eq!(
        me_status(&state, &alice_cookie).await,
        StatusCode::UNAUTHORIZED,
        "absolute-TTL gate should reject the old session"
    );
    let remaining: i64 = tokio::task::spawn_blocking({
        let db = db.clone();
        move || {
            let conn = db.conn().blocking_lock();
            conn.query_row(
                "SELECT COUNT(*) FROM sessions WHERE user_id = (SELECT id FROM users WHERE username='alice')",
                [],
                |r| r.get(0),
            )
            .unwrap()
        }
    })
    .await
    .unwrap();
    assert_eq!(remaining, 0, "stale session row should be lazily deleted");

    clear_test_now_unix();
}

/// G/W/T 5.8: logout still deletes the session row.
#[tokio::test]
async fn logout_deletes_session_row() {
    let _g = SERIAL.lock().await;
    clear_test_now_unix();

    let state = test_state_with_db();
    let admin = register_and_login(&state).await;
    create_user(&state, &admin, "alice", "alice_passWORD1!").await;
    let cookie = login_with_username(&state, "alice", "alice_passWORD1!").await;

    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::post("/api/auth/logout")
                .header("Origin", TEST_ORIGIN)
                .header("Cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let db = state.auth.db.as_ref().unwrap().clone();
    let alice_rows: i64 = tokio::task::spawn_blocking(move || {
        let conn = db.conn().blocking_lock();
        conn.query_row(
            "SELECT COUNT(*) FROM sessions WHERE user_id = (SELECT id FROM users WHERE username='alice')",
            [],
            |r| r.get(0),
        )
        .unwrap()
    })
    .await
    .unwrap();
    assert_eq!(alice_rows, 0);
}

/// Make sure `Config::default()` agrees that the kick flag defaults to true,
/// so a deployment that doesn't override it gets the security-first behaviour.
#[test]
fn config_default_kicks_other_sessions() {
    let cfg = Config::default();
    assert!(cfg.web.kick_other_sessions_on_login);
}
