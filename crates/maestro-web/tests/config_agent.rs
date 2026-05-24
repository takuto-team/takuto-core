// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

// Copyright (C) 2026 Alexandre Obellianne
//
// Phase 1 integration tests for PUT /api/config/agent and the
// `provider_changed` WebSocket event. Source: tmp/multi-agents/04_architecture.md
// §2.3.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower::ServiceExt;

use maestro_core::config::AiAgentProvider;
use maestro_core::config_writer::ConfigWriter;
use maestro_web::server::build_router;
use maestro_web::test_helpers::{register_and_login, test_state_with_db};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Promote a non-admin user to user role and return their session cookie.
async fn create_regular_user_login(state: &maestro_web::state::AppState, admin_cookie: &str) -> String {
    let app = build_router(state.clone());
    let create_resp = app
        .oneshot(
            Request::post("/api/users")
                .header("Content-Type", "application/json")
                .header("Origin", "http://localhost:8080")
                .header("Cookie", admin_cookie)
                .body(Body::from(
                    r#"{"username":"alice","password":"testpassword1234","role":"user"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create_resp.status(), StatusCode::CREATED);

    let app = build_router(state.clone());
    let login_resp = app
        .oneshot(
            Request::post("/api/auth/login")
                .header("Content-Type", "application/json")
                .header("Origin", "http://localhost:8080")
                .body(Body::from(
                    r#"{"username":"alice","password":"testpassword1234"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login_resp.status(), StatusCode::NO_CONTENT);
    let set_cookie = login_resp
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    set_cookie.split(';').next().unwrap().trim().to_string()
}

// ---------------------------------------------------------------------------
// Test cases
// ---------------------------------------------------------------------------

#[tokio::test]
async fn put_config_agent_as_admin_updates_and_persists() {
    // Wire a real ConfigWriter against a temp config.toml so the persistence
    // path is exercised end-to-end.
    let dir = std::env::temp_dir().join(format!("maestro-config-agent-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let config_path = dir.join("config.toml");
    std::fs::write(&config_path, "").unwrap();

    let mut state = test_state_with_db();
    state.config_mut().config_writer = Some(Arc::new(ConfigWriter::new(config_path.clone())));
    state.config_mut().config_path = config_path.clone();

    let cookie = register_and_login(&state).await;
    let app = build_router(state.clone());

    let resp = app
        .oneshot(
            Request::put("/api/config/agent")
                .header("Content-Type", "application/json")
                .header("Origin", "http://localhost:8080")
                .header("Cookie", &cookie)
                .body(Body::from(
                    r#"{
                        "providers": {
                            "claude": {
                                "model": "claude-3-5-sonnet-latest",
                                "base_url": "https://proxy.example.com",
                                "extra_args": ["--max-turns", "50"]
                            }
                        }
                    }"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["persisted"], true);
    assert_eq!(
        json["agent"]["providers"]["claude"]["model"],
        "claude-3-5-sonnet-latest"
    );

    // Verify the in-memory config got patched.
    let cfg = state.config().config.read().await;
    assert_eq!(cfg.agent.providers.claude.model, "claude-3-5-sonnet-latest");
    assert_eq!(
        cfg.agent.providers.claude.base_url,
        "https://proxy.example.com"
    );
    assert_eq!(
        cfg.agent.providers.claude.extra_args,
        vec!["--max-turns".to_string(), "50".to_string()]
    );

    // And that the disk file was written.
    let on_disk = std::fs::read_to_string(&config_path).unwrap();
    assert!(on_disk.contains("claude-3-5-sonnet-latest"));
}

/// Task #38: when ConfigWriter has had to fall back to the in-place write
/// path (`used_inplace_fallback` flag set), the next refresh of
/// `system_status` from PUT /api/config/agent must include an info-level
/// `config_file_bind_mounted` diagnostic. The dashboard reads
/// `/api/onboarding/status` afterwards and surfaces it as a non-critical
/// banner. We don't construct a real Linux bind mount in this test; we
/// just latch the flag directly via the public accessor and verify the
/// route handler picks it up.
#[tokio::test]
async fn put_config_agent_emits_config_file_bind_mounted_when_writer_flag_set() {
    use std::sync::atomic::Ordering;

    let dir = std::env::temp_dir().join(format!("maestro-task-38-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let config_path = dir.join("config.toml");
    std::fs::write(&config_path, "").unwrap();

    let mut state = test_state_with_db();
    let writer = Arc::new(ConfigWriter::new(config_path.clone()));
    // Latch the flag BEFORE the request so the refresh path sees it. In
    // production the writer would set this itself after an EBUSY fallback;
    // exposing it via the public Arc<AtomicBool> accessor keeps the test
    // independent of forcing a real Linux bind mount.
    writer.used_inplace_fallback().store(true, Ordering::Release);
    state.config_mut().config_writer = Some(writer);
    state.config_mut().config_path = config_path.clone();

    let cookie = register_and_login(&state).await;
    let app = build_router(state.clone());

    let resp = app
        .oneshot(
            Request::put("/api/config/agent")
                .header("Content-Type", "application/json")
                .header("Origin", "http://localhost:8080")
                .header("Cookie", &cookie)
                .body(Body::from(
                    r#"{"providers":{"claude":{"model":"claude-3-5-sonnet-latest"}}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // The handler refreshed system_status; the new warning must be there.
    let snapshot = state.engine().system_status.read().await.clone();
    let bind_mount_warning = snapshot
        .warnings
        .iter()
        .find(|w| w.code == "config_file_bind_mounted");
    let w = bind_mount_warning
        .expect("system_status must carry config_file_bind_mounted after fallback flag latched");
    assert_eq!(
        w.severity, "info",
        "config_file_bind_mounted must be info-severity (saves succeeded)"
    );
}

#[tokio::test]
async fn put_config_agent_as_non_admin_returns_403_no_side_effects() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;
    let alice_cookie = create_regular_user_login(&state, &admin_cookie).await;

    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::put("/api/config/agent")
                .header("Content-Type", "application/json")
                .header("Origin", "http://localhost:8080")
                .header("Cookie", &alice_cookie)
                .body(Body::from(
                    r#"{"providers":{"claude":{"model":"hack"}}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    // Confirm the config was not mutated.
    let cfg = state.config().config.read().await;
    assert_eq!(cfg.agent.providers.claude.model, "");
}

#[tokio::test]
async fn put_config_agent_denied_extra_arg_returns_400_no_side_effects() {
    let state = test_state_with_db();
    let cookie = register_and_login(&state).await;

    // Capture the pre-state for comparison.
    let pre = {
        let cfg = state.config().config.read().await;
        cfg.agent.providers.claude.extra_args.clone()
    };

    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::put("/api/config/agent")
                .header("Content-Type", "application/json")
                .header("Origin", "http://localhost:8080")
                .header("Cookie", &cookie)
                .body(Body::from(
                    r#"{"providers":{"claude":{"extra_args":["--dangerously-skip-permissions"]}}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let msg = std::str::from_utf8(&body).unwrap();
    assert!(
        msg.contains("extra_args_denied"),
        "error should carry stable code, got: {msg}"
    );

    // Confirm extra_args was not mutated.
    let cfg = state.config().config.read().await;
    assert_eq!(cfg.agent.providers.claude.extra_args, pre);
}

#[tokio::test]
async fn put_config_agent_unknown_top_level_key_returns_400() {
    let state = test_state_with_db();
    let cookie = register_and_login(&state).await;

    let app = build_router(state);
    let resp = app
        .oneshot(
            Request::put("/api/config/agent")
                .header("Content-Type", "application/json")
                .header("Origin", "http://localhost:8080")
                .header("Cookie", &cookie)
                .body(Body::from(r#"{"bogus_key":42}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    // T-ADMIN-004 / Phase 1 fixup: the handler accepts a serde_json::Value
    // and re-deserializes with deny_unknown_fields so the rejection lands as
    // 400 Bad Request (matching PUT /api/config), not axum's default 422.
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Body carries the stable error code for the UI to switch on.
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"], "unknown_field_or_invalid_shape");
}

#[tokio::test]
async fn put_config_agent_changing_provider_broadcasts_provider_changed_event() {
    let dir = std::env::temp_dir().join(format!("maestro-config-agent-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let config_path = dir.join("config.toml");
    std::fs::write(&config_path, "").unwrap();

    let mut state = test_state_with_db();
    state.config_mut().config_writer = Some(Arc::new(ConfigWriter::new(config_path.clone())));
    state.config_mut().config_path = config_path.clone();

    let cookie = register_and_login(&state).await;

    // Subscribe BEFORE sending the request so we don't miss the broadcast.
    let mut rx = state.engine().engine.subscribe();

    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::put("/api/config/agent")
                .header("Content-Type", "application/json")
                .header("Origin", "http://localhost:8080")
                .header("Cookie", &cookie)
                .body(Body::from(r#"{"provider":"cursor"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Drain events until we see the provider_changed one (or time out).
    let evt = tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            match rx.recv().await {
                Ok(e) if e.event_type == "provider_changed" => break Some(e),
                Ok(_) => continue,
                Err(_) => break None,
            }
        }
    })
    .await
    .expect("timed out waiting for provider_changed event")
    .expect("broadcast channel closed");

    assert_eq!(evt.provider_from.as_deref(), Some("claude"));
    assert_eq!(evt.provider_to.as_deref(), Some("cursor"));
    assert_eq!(evt.affected_users.as_deref(), Some(&[][..]));

    // In-memory state reflects the new provider.
    let cfg = state.config().config.read().await;
    assert_eq!(cfg.agent.provider, AiAgentProvider::Cursor);
}

#[tokio::test]
async fn put_config_agent_same_provider_does_not_broadcast() {
    let dir = std::env::temp_dir().join(format!("maestro-config-agent-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let config_path = dir.join("config.toml");
    std::fs::write(&config_path, "").unwrap();

    let mut state = test_state_with_db();
    state.config_mut().config_writer = Some(Arc::new(ConfigWriter::new(config_path.clone())));
    state.config_mut().config_path = config_path.clone();

    let cookie = register_and_login(&state).await;
    let mut rx = state.engine().engine.subscribe();

    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::put("/api/config/agent")
                .header("Content-Type", "application/json")
                .header("Origin", "http://localhost:8080")
                .header("Cookie", &cookie)
                .body(Body::from(r#"{"provider":"claude"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Watch for a provider_changed event within a short window — it must NOT arrive.
    let timed = tokio::time::timeout(std::time::Duration::from_millis(200), async {
        loop {
            match rx.recv().await {
                Ok(e) if e.event_type == "provider_changed" => break Some(e),
                Ok(_) => continue,
                Err(_) => break None,
            }
        }
    })
    .await;
    // Timeout = no provider_changed event was received — that's the happy path.
    assert!(
        timed.is_err(),
        "expected no provider_changed broadcast when provider was unchanged"
    );
}

#[tokio::test]
async fn put_config_agent_unknown_provider_value_returns_400() {
    let state = test_state_with_db();
    let cookie = register_and_login(&state).await;

    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::put("/api/config/agent")
                .header("Content-Type", "application/json")
                .header("Origin", "http://localhost:8080")
                .header("Cookie", &cookie)
                .body(Body::from(r#"{"provider":"gemini"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Provider unchanged.
    let cfg = state.config().config.read().await;
    assert_eq!(cfg.agent.provider, AiAgentProvider::Claude);
}

/// Phase 1 fixup AC-4: after PUT /api/config/agent changes the active
/// provider, GET /api/auth/status reflects the new `provider_selected` in
/// the same process (no restart required). Verifies the in-memory
/// `system_status` refresh hooked into the PUT handler.
#[tokio::test]
async fn put_config_agent_refreshes_system_status_for_auth_status() {
    let dir = std::env::temp_dir().join(format!("maestro-config-agent-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let config_path = dir.join("config.toml");
    std::fs::write(&config_path, "").unwrap();

    let mut state = test_state_with_db();
    state.config_mut().config_writer = Some(Arc::new(ConfigWriter::new(config_path.clone())));
    state.config_mut().config_path = config_path.clone();

    let cookie = register_and_login(&state).await;

    // Sanity: pre-state reports claude.
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::get("/api/auth/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        json["provider_selected"], "claude",
        "pre-state should be claude"
    );

    // Switch provider to cursor.
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::put("/api/config/agent")
                .header("Content-Type", "application/json")
                .header("Origin", "http://localhost:8080")
                .header("Cookie", &cookie)
                .body(Body::from(r#"{"provider":"cursor"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // The mirrored field on /api/auth/status must now report "cursor" —
    // proves the in-memory refresh hooked the auth_status read path.
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::get("/api/auth/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        json["provider_selected"], "cursor",
        "after PUT, auth_status should mirror the new provider"
    );

    // And /api/onboarding/status also reflects the new value.
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::get("/api/onboarding/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        json["provider"]["selected"], "cursor",
        "after PUT, onboarding/status should reflect the new provider"
    );
}

// Silence unused-import lint when this file's helpers grow.
#[allow(dead_code)]
fn _dummy(_x: Arc<RwLock<()>>) {}
