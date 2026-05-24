// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

// Copyright (C) 2026 Alexandre Obellianne
//
// Integration test for plan-02 AC-4 — Argon2id parameters + rehash-on-login.
//
// Verifies the end-to-end flow: a credential row that stores a legacy
// `Argon2::default()` hash is silently rewritten to the current (stronger)
// parameters when its owner successfully logs in.
//
// Uses the public `legacy_argon2_default_hash_for_tests` helper exposed from
// `maestro_core::db::credentials` so this test file does not need to depend on
// the `argon2` / `getrandom` crates directly.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use rusqlite::params;
use tower::ServiceExt;

use maestro_core::db::credentials::legacy_argon2_default_hash_for_tests;
use maestro_web::server::build_router;
use maestro_web::test_helpers::{TEST_ORIGIN, register_and_login, test_state_with_db};

#[tokio::test]
async fn successful_login_rehashes_legacy_argon2_default_hash() {
    let state = test_state_with_db();
    // Register `admin` then immediately downgrade their stored hash to legacy
    // params via direct SQL — simulating an old credential row.
    let _ = register_and_login(&state).await;

    let db = state.auth().db.clone().expect("test state has a db");
    let legacy = legacy_argon2_default_hash_for_tests("testpassword1234")
        .expect("legacy argon2 hash helper");
    let legacy_clone = legacy.clone();
    let (cred_id, user_id) = tokio::task::spawn_blocking(move || {
        let conn = db.conn().blocking_lock();
        // Replace the existing admin's password credential with the legacy hash.
        let (cred_id, user_id): (String, String) = conn
            .query_row(
                "SELECT c.id, c.user_id FROM credentials c \
                 JOIN users u ON u.id = c.user_id \
                 WHERE u.username = 'admin' AND c.kind = 'password' LIMIT 1",
                [],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .unwrap();
        conn.execute(
            "UPDATE credentials SET data = ?1 WHERE id = ?2",
            params![legacy_clone, cred_id],
        )
        .unwrap();
        (cred_id, user_id)
    })
    .await
    .unwrap();

    // Confirm the row now uses legacy params.
    let stored: Vec<u8> = {
        let db = state.auth().db.clone().expect("db");
        let cred_id = cred_id.clone();
        tokio::task::spawn_blocking(move || {
            let conn = db.conn().blocking_lock();
            conn.query_row(
                "SELECT data FROM credentials WHERE id = ?1",
                params![cred_id],
                |r| r.get(0),
            )
            .unwrap()
        })
        .await
        .unwrap()
    };
    assert_eq!(stored, legacy, "pre-condition: row should hold legacy hash");

    // Drive a real login through the router. The verify path must succeed and
    // the rehash side-effect must rewrite `data`.
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::post("/api/auth/login")
                .header("Content-Type", "application/json")
                .header("Origin", TEST_ORIGIN)
                .body(Body::from(
                    r#"{"username":"admin","password":"testpassword1234"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT, "login must succeed");

    // The row should now hold a fresh, current-params hash.
    let after: Vec<u8> = {
        let db = state.auth().db.clone().expect("db");
        let cred_id = cred_id.clone();
        tokio::task::spawn_blocking(move || {
            let conn = db.conn().blocking_lock();
            conn.query_row(
                "SELECT data FROM credentials WHERE id = ?1",
                params![cred_id],
                |r| r.get(0),
            )
            .unwrap()
        })
        .await
        .unwrap()
    };
    assert_ne!(legacy, after, "credentials.data should have been rewritten after login");

    // The PHC string format embeds the params textually as
    // `$argon2id$v=19$m=47104,t=1,p=1$<salt>$<hash>` — assert directly without
    // pulling the `argon2` crate into this test's dependency surface.
    let after_str = std::str::from_utf8(&after).unwrap();
    assert!(
        after_str.starts_with("$argon2id$"),
        "rehashed hash must be Argon2id, got: {after_str}",
    );
    assert!(
        after_str.contains("$m=47104,t=1,p=1$"),
        "rehashed hash must embed current password params, got: {after_str}",
    );
    // And the legacy default's params must no longer be present.
    assert!(
        !after_str.contains("$m=19456,t=2,p=1$"),
        "rehashed hash should no longer embed the legacy default params, got: {after_str}",
    );

    // And the user can still log in (the rehash produced a verifying hash).
    let app = build_router(state.clone());
    let resp2 = app
        .oneshot(
            Request::post("/api/auth/login")
                .header("Content-Type", "application/json")
                .header("Origin", TEST_ORIGIN)
                .body(Body::from(
                    r#"{"username":"admin","password":"testpassword1234"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp2.status(), StatusCode::NO_CONTENT);

    // Suppress unused warnings while keeping the variable for debugging clarity.
    let _ = user_id;
}

#[tokio::test]
async fn current_param_hash_is_not_rewritten_on_login() {
    let state = test_state_with_db();
    let _ = register_and_login(&state).await;

    let db = state.auth().db.clone().expect("db");
    let cred_id: String = tokio::task::spawn_blocking(move || {
        let conn = db.conn().blocking_lock();
        conn.query_row(
            "SELECT c.id FROM credentials c \
             JOIN users u ON u.id = c.user_id \
             WHERE u.username = 'admin' AND c.kind = 'password' LIMIT 1",
            [],
            |r| r.get::<_, String>(0),
        )
        .unwrap()
    })
    .await
    .unwrap();

    // Snapshot the current-params hash before login.
    let db = state.auth().db.clone().expect("db");
    let cred_id_for_before = cred_id.clone();
    let before: Vec<u8> = tokio::task::spawn_blocking(move || {
        let conn = db.conn().blocking_lock();
        conn.query_row(
            "SELECT data FROM credentials WHERE id = ?1",
            params![cred_id_for_before],
            |r| r.get(0),
        )
        .unwrap()
    })
    .await
    .unwrap();

    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::post("/api/auth/login")
                .header("Content-Type", "application/json")
                .header("Origin", TEST_ORIGIN)
                .body(Body::from(
                    r#"{"username":"admin","password":"testpassword1234"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let db = state.auth().db.clone().expect("db");
    let cred_id_for_after = cred_id.clone();
    let after: Vec<u8> = tokio::task::spawn_blocking(move || {
        let conn = db.conn().blocking_lock();
        conn.query_row(
            "SELECT data FROM credentials WHERE id = ?1",
            params![cred_id_for_after],
            |r| r.get(0),
        )
        .unwrap()
    })
    .await
    .unwrap();

    assert_eq!(
        before, after,
        "current-param hash must not be rewritten on login"
    );
}
