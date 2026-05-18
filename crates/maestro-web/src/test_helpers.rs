// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Shared test utilities for `maestro-web` integration tests.
//!
//! Every test that creates an `AppState` must use a real (temp-dir) SQLite
//! database so that the auth middleware can validate sessions.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tokio::sync::RwLock;
use tower::ServiceExt;

use maestro_core::actions::dry_run::DryRunActions;
use maestro_core::config::{Config, TicketingSystem};
use maestro_core::db::Database;
use maestro_core::workflow::engine::WorkflowEngine;

use crate::server::build_router;
use crate::state::AppState;

/// Create a fresh [`Database`] backed by a unique temp directory.
///
/// Each call produces an isolated database so tests never interfere with each
/// other. Migrations are applied automatically by `Database::open`.
pub fn temp_db() -> Database {
    let dir = std::env::temp_dir().join(format!("maestro-test-{}", uuid::Uuid::new_v4()));
    Database::open(&dir).expect("failed to create temp test database")
}

/// Create a test [`AppState`] with a temp SQLite database (no users registered yet).
///
/// The returned state has `db: Some(...)` so the auth middleware is active.
/// Call [`register_and_login`] to create the first admin user and obtain
/// a session cookie.
pub fn test_state_with_db() -> AppState {
    let db = temp_db();
    test_state_with_db_instance(db)
}

/// Create a test [`AppState`] using a pre-created [`Database`].
///
/// Useful when you need to tweak the config or insert data into the DB before
/// constructing the state.
pub fn test_state_with_db_instance(db: Database) -> AppState {
    let config = Arc::new(RwLock::new(Config::default()));
    let actions: Arc<dyn maestro_core::actions::traits::ExternalActions> = Arc::new(
        DryRunActions::new("origin".to_string(), None),
    );
    let jira_available = Arc::new(AtomicBool::new(false));
    let engine = Arc::new(WorkflowEngine::new(
        config.clone(),
        actions,
        1,
        jira_available.clone(),
        TicketingSystem::None,
        std::env::temp_dir(),
    ));
    AppState {
        engine,
        config,
        db: Some(db),
        polling_paused: Arc::new(AtomicBool::new(false)),
        jira_available,
        ticketing_system: TicketingSystem::None,
        editor_scanners: Arc::new(RwLock::new(HashMap::new())),
        dynamic_forwards: Arc::new(RwLock::new(HashMap::new())),
        terminal_ports: Arc::new(RwLock::new(HashMap::new())),
        run_commands: Arc::new(RwLock::new(HashMap::new())),
        preflight_error: None,
        system_status: std::sync::Arc::new(tokio::sync::RwLock::new(maestro_core::docker_hooks::SystemStatus::default())),
        config_path: std::env::temp_dir().join("config.toml"),
        config_writer: None,
        clone_in_progress: Arc::new(AtomicBool::new(false)),
        path_token_registry: crate::session_registry::PathTokenRegistry::new(),
    }
}

/// Origin header to attach to mutating requests in tests. Matches the
/// auto-computed `cors_origins` allowlist of `WebConfig::default()`
/// (host=`0.0.0.0`, port=8080 → `http://localhost:8080` is allowed). The CSRF
/// middleware (plan-02 AC-1) rejects mutating requests whose `Origin` is not
/// on the allowlist, so every test that POSTs/PUTs/DELETEs/PATCHes must
/// either send this header (when authenticated) or omit it (to assert that
/// CSRF rejects the request).
pub const TEST_ORIGIN: &str = "http://localhost:8080";

/// Register the first admin user and log in, returning the session cookie string.
///
/// The cookie is in `name=value` form (e.g. `maestro_session=db-<uuid>`) ready
/// for use in a `Cookie` header.
///
/// Username: `admin`, password: `testpassword1234`.
pub async fn register_and_login(state: &AppState) -> String {
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::post("/api/auth/register")
                .header("Content-Type", "application/json")
                .header("Origin", TEST_ORIGIN)
                .body(Body::from(
                    r#"{"username":"admin","password":"testpassword1234"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "register should succeed"
    );

    let app = build_router(state.clone());
    let login_resp = app
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
