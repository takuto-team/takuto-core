// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

// Test-only `std::env` mutation (unsafe in the 2024 edition); serialised within the test.
#![allow(unsafe_code)]

// Copyright (C) 2026 Alexandre Obellianne
//
// Integration tests for the per-user repository REST endpoints.
//
//   GET    /api/repositories
//   GET    /api/repositories/_available
//   POST   /api/repositories
//   DELETE /api/repositories/{id}
//
// Clone-side tests are unit-tested at the validator level and excluded
// here — the real clone path shells out to `git`/`gh` and requires network
// access, which makes it unfit for hermetic integration tests. We exercise
// the `{repo_url}` branch up to the point of the URL-already-known
// short-circuit and the URL-validation rejection matrix.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::Value;
use tokio::sync::RwLock;
use tower::ServiceExt;

use maestro_core::actions::dry_run::DryRunActions;
use maestro_core::config::{Config, TicketingSystem};
use maestro_core::db::Database;
use maestro_core::workflow::engine::WorkflowEngine;
use maestro_web::server::build_router;
use maestro_web::state::AppState;
use maestro_web::test_helpers::{TEST_ORIGIN, register_and_login};

/// Simple RAII temp-dir replacement — we can't use `tempfile` (not in deps).
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("maestro-repos-it-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p).expect("create temp dir");
        Self { path: p }
    }
    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

// ---------------------------------------------------------------------------
// Plumbing — temp data_dir + temp workspaces_dir, isolated per test.
// ---------------------------------------------------------------------------

/// Build a fresh `AppState` rooted in an isolated temp dir for each test.
/// We don't use `test_state_with_db` because we want a deterministic
/// `data_dir` so the `repository_has_active_workflow` scanner has a stable
/// floor (empty by default — no workflows on disk).
fn test_state_isolated() -> (AppState, TempDir) {
    let dir = TempDir::new();
    let db = Database::open(dir.path(), true).expect("open db");
    let config = Arc::new(RwLock::new(Config::default()));
    let actions: Arc<dyn maestro_core::actions::traits::ExternalActions> =
        Arc::new(DryRunActions::new("origin".to_string(), None));
    let jira_available = Arc::new(AtomicBool::new(false));
    let engine = Arc::new(WorkflowEngine::new(
        config.clone(),
        actions,
        1,
        jira_available.clone(),
        TicketingSystem::None,
        dir.path().to_path_buf(),
    ));
    let git_auth_resolver = Some(Arc::new(
        maestro_core::github::auth_resolver::GitAuthResolver::new(db.clone(), None),
    ));
    use maestro_web::state::{AuthState, ConfigState, EditorState, EngineState, RunCommandState};
    let state = AppState::new(
        EngineState {
            engine,
            polling_paused: Arc::new(AtomicBool::new(false)),
            clone_in_progress: Arc::new(AtomicBool::new(false)),
            system_status: Arc::new(RwLock::new(
                maestro_core::docker_hooks::SystemStatus::default(),
            )),
        },
        AuthState {
            db: Some(db),
            gh_client: Arc::new(maestro_core::auth::RealGhClient::new()),
            git_auth_resolver,
        },
        ConfigState {
            config,
            config_path: dir.path().join("config.toml"),
            config_writer: None,
            ticketing_system: TicketingSystem::None,
            jira_available,
            preflight_error: None,
            work_item_flow_defaults: std::sync::Arc::new(Vec::new()),
        },
        EditorState {
            editor_scanners: Arc::new(RwLock::new(std::collections::HashMap::new())),
            dynamic_forwards: Arc::new(RwLock::new(std::collections::HashMap::new())),
            terminal_ports: Arc::new(RwLock::new(std::collections::HashMap::new())),
            editor_bundles: Arc::new(RwLock::new(std::collections::HashMap::new())),
            path_token_registry: maestro_web::session_registry::PathTokenRegistry::new(),
        },
        RunCommandState {
            run_commands: Arc::new(RwLock::new(std::collections::HashMap::new())),
            run_command_bundles: Arc::new(RwLock::new(std::collections::HashMap::new())),
        },
    );
    (state, dir)
}

/// Register the first admin user and log in, returning the cookie.
async fn register_admin(state: &AppState) -> String {
    register_and_login(state).await
}

/// Create a second user (non-admin) via the admin /api/users endpoint and log
/// them in. Returns their session cookie.
async fn create_user_login(
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
        .expect("set-cookie")
        .to_str()
        .unwrap()
        .to_string();
    set_cookie.split(';').next().unwrap().trim().to_string()
}

/// Seed a `repositories` row directly via the DB module so we don't need to
/// run a real `git clone` to exercise the user-association endpoints.
async fn seed_repository(
    state: &AppState,
    name: &str,
    repo_url: Option<&str>,
    local_path: &str,
) -> String {
    let db = state.auth().db.as_ref().expect("db");
    maestro_core::db::repositories::upsert(db.adapter(), name, repo_url, local_path, "main", None)
        .await
        .expect("seed repository")
}

async fn list_mine(state: &AppState, cookie: &str) -> (StatusCode, Value) {
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::get("/api/repositories")
                .header("Cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    (status, json)
}

async fn list_available(state: &AppState, cookie: &str) -> (StatusCode, Value) {
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::get("/api/repositories/_available")
                .header("Cookie", cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    (status, json)
}

async fn post_repositories(state: &AppState, cookie: &str, body_json: &str) -> (StatusCode, Value) {
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::post("/api/repositories")
                .header("Content-Type", "application/json")
                .header("Origin", TEST_ORIGIN)
                .header("Cookie", cookie)
                .body(Body::from(body_json.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&body).into_owned();
    let json: Value = serde_json::from_slice(&body).unwrap_or(Value::String(text));
    (status, json)
}

async fn delete_repository(
    state: &AppState,
    cookie: &str,
    id: &str,
    body: Option<&str>,
) -> StatusCode {
    let app = build_router(state.clone());
    let req = Request::builder()
        .method("DELETE")
        .uri(format!("/api/repositories/{id}"))
        .header("Origin", TEST_ORIGIN)
        .header("Cookie", cookie);
    let req = if let Some(b) = body {
        req.header("Content-Type", "application/json")
            .body(Body::from(b.to_string()))
            .unwrap()
    } else {
        req.body(Body::empty()).unwrap()
    };
    let resp = app.oneshot(req).await.unwrap();
    resp.status()
}

// ---------------------------------------------------------------------------
// Empty state.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac1_freshly_created_user_sees_empty_list() {
    let (state, _tmp) = test_state_isolated();
    let cookie = register_admin(&state).await;

    let (status, json) = list_mine(&state, &cookie).await;
    assert_eq!(status, StatusCode::OK);
    assert!(json.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn ac1_legacy_workspaces_endpoints_are_gone() {
    // The endpoints are dropped from the API router. Requests fall through to
    // the SPA fallback (which serves text/html), not to the deleted handlers.
    // We assert "not API JSON" — any of: 404 / 405 / HTML SPA response.
    let (state, _tmp) = test_state_isolated();
    let cookie = register_admin(&state).await;

    for (method, uri) in [
        ("GET", "/api/workspaces"),
        ("POST", "/api/workspaces/switch"),
        ("POST", "/api/repos/clone"),
    ] {
        let app = build_router(state.clone());
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .header("Origin", TEST_ORIGIN)
            .header("Cookie", &cookie)
            .header("Content-Type", "application/json")
            .body(Body::from("{}"))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let ct = resp
            .headers()
            .get("content-type")
            .map(|v| v.to_str().unwrap_or("").to_string())
            .unwrap_or_default();
        assert!(
            !ct.contains("application/json"),
            "{method} {uri} must NOT respond with API JSON (legacy endpoint hard-deleted) — got content-type {ct}"
        );
    }
}

#[tokio::test]
async fn unauthenticated_caller_gets_401() {
    let (state, _tmp) = test_state_isolated();
    let app = build_router(state);
    let resp = app
        .oneshot(
            Request::get("/api/repositories")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ---------------------------------------------------------------------------
// List, add, isolation.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac3_add_by_repository_id_is_idempotent() {
    let (state, _tmp) = test_state_isolated();
    let cookie = register_admin(&state).await;
    let repo_id = seed_repository(&state, "alpha", None, "/workspaces/alpha").await;

    // First add → 200 with the row.
    let body = format!(r#"{{"repository_id":"{repo_id}"}}"#);
    let (status, json) = post_repositories(&state, &cookie, &body).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["id"], repo_id);
    assert_eq!(json["name"], "alpha");

    // Second add → still 200; my list still has 1 entry.
    let (status, _) = post_repositories(&state, &cookie, &body).await;
    assert_eq!(status, StatusCode::OK);

    let (_, mine) = list_mine(&state, &cookie).await;
    assert_eq!(mine.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn ac2_available_excludes_already_added() {
    let (state, _tmp) = test_state_isolated();
    let cookie = register_admin(&state).await;
    let r1 = seed_repository(&state, "r1", None, "/workspaces/r1").await;
    let r2 = seed_repository(&state, "r2", None, "/workspaces/r2").await;

    // Before adding any: both available.
    let (_, avail) = list_available(&state, &cookie).await;
    let avail_ids: Vec<&str> = avail
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert_eq!(avail_ids.len(), 2);
    assert!(avail_ids.contains(&r1.as_str()));
    assert!(avail_ids.contains(&r2.as_str()));

    // Add r1.
    let body = format!(r#"{{"repository_id":"{r1}"}}"#);
    post_repositories(&state, &cookie, &body).await;

    // Now only r2 is available.
    let (_, avail) = list_available(&state, &cookie).await;
    let avail_ids: Vec<&str> = avail
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert_eq!(avail_ids, vec![r2.as_str()]);
}

#[tokio::test]
async fn ac3_add_unknown_repository_id_returns_404() {
    let (state, _tmp) = test_state_isolated();
    let cookie = register_admin(&state).await;

    let (status, _) =
        post_repositories(&state, &cookie, r#"{"repository_id":"does-not-exist"}"#).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn ac10_user_a_cannot_see_user_b_associations() {
    let (state, _tmp) = test_state_isolated();
    let admin_cookie = register_admin(&state).await;
    let bob_cookie = create_user_login(&state, &admin_cookie, "bob", "bobpassword1234").await;

    let r1 = seed_repository(&state, "secret-repo", None, "/workspaces/secret-repo").await;

    // Admin adds the repo.
    let body = format!(r#"{{"repository_id":"{r1}"}}"#);
    post_repositories(&state, &admin_cookie, &body).await;

    // Bob's list is empty.
    let (_, mine) = list_mine(&state, &bob_cookie).await;
    assert!(mine.as_array().unwrap().is_empty());

    // Bob's `_available` includes the repo (deployment-wide visibility per G10).
    let (_, avail) = list_available(&state, &bob_cookie).await;
    assert!(
        avail.as_array().unwrap().iter().any(|r| r["id"] == r1),
        "available list must show deployment-wide repos"
    );
}

#[tokio::test]
async fn ac6_non_admin_can_post_repositories_with_repository_id() {
    // Cloning needs a real remote; we settle for verifying that
    // `POST /api/repositories` with `repository_id` succeeds for a
    // non-admin caller, which proves the route has no admin gate.
    let (state, _tmp) = test_state_isolated();
    let admin_cookie = register_admin(&state).await;
    let alice_cookie = create_user_login(&state, &admin_cookie, "alice", "alicepassword1234").await;

    let r1 = seed_repository(&state, "shared", None, "/workspaces/shared").await;
    let body = format!(r#"{{"repository_id":"{r1}"}}"#);

    let (status, _) = post_repositories(&state, &alice_cookie, &body).await;
    assert_eq!(status, StatusCode::OK, "non-admin must be allowed to add");
}

// ---------------------------------------------------------------------------
// Last-user remove + on-disk purge.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac7_last_user_remove_purges_disk() {
    let (state, tmp) = test_state_isolated();
    let cookie = register_admin(&state).await;

    // Create a real on-disk directory we own — the handler will rmrf it on purge.
    let local_path = tmp.path().join("workspaces").join("burnable");
    std::fs::create_dir_all(local_path.join(".git")).expect("create fake clone");
    std::fs::write(
        local_path.join(".git").join("config"),
        "[core]\nrepositoryformatversion = 0\n",
    )
    .unwrap();
    let local_path_str = local_path.to_string_lossy().into_owned();

    let r1 = seed_repository(&state, "burnable", None, &local_path_str).await;

    // Admin adds the repo.
    let body = format!(r#"{{"repository_id":"{r1}"}}"#);
    let (status, _) = post_repositories(&state, &cookie, &body).await;
    assert_eq!(status, StatusCode::OK);

    // Admin removes — they were the only user → purge.
    let status = delete_repository(&state, &cookie, &r1, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // The on-disk directory is gone.
    assert!(!local_path.exists(), "on-disk clone must be purged");

    // The DB row is gone.
    let db = state.auth().db.as_ref().unwrap();
    let row = maestro_core::db::repositories::get(db.adapter(), &r1)
        .await
        .unwrap();
    assert!(row.is_none(), "DB row must be deleted");
}

#[tokio::test]
async fn ac9_non_last_user_remove_keeps_disk_and_row() {
    let (state, tmp) = test_state_isolated();
    let admin_cookie = register_admin(&state).await;
    let bob_cookie = create_user_login(&state, &admin_cookie, "bob", "bobpassword1234").await;

    let local_path = tmp.path().join("workspaces").join("shared-clone");
    std::fs::create_dir_all(&local_path).expect("create dir");
    let local_path_str = local_path.to_string_lossy().into_owned();

    let r1 = seed_repository(&state, "shared-clone", None, &local_path_str).await;

    // Both users add.
    let body = format!(r#"{{"repository_id":"{r1}"}}"#);
    post_repositories(&state, &admin_cookie, &body).await;
    post_repositories(&state, &bob_cookie, &body).await;

    // Admin removes — bob still has it.
    let status = delete_repository(&state, &admin_cookie, &r1, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // On-disk + DB row preserved.
    assert!(local_path.exists(), "shared clone must NOT be purged");
    let db = state.auth().db.as_ref().unwrap();
    let row = maestro_core::db::repositories::get(db.adapter(), &r1)
        .await
        .unwrap();
    assert!(row.is_some(), "DB row must be preserved");

    // Admin's list is now empty; bob still has it.
    let (_, admin_mine) = list_mine(&state, &admin_cookie).await;
    assert!(admin_mine.as_array().unwrap().is_empty());
    let (_, bob_mine) = list_mine(&state, &bob_cookie).await;
    assert_eq!(bob_mine.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn delete_unknown_repository_returns_404_for_non_purge() {
    let (state, _tmp) = test_state_isolated();
    let cookie = register_admin(&state).await;
    let status = delete_repository(&state, &cookie, "does-not-exist", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// Admin force_purge.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac8_admin_force_purge_drops_row_for_everyone() {
    let (state, tmp) = test_state_isolated();
    let admin_cookie = register_admin(&state).await;
    let bob_cookie = create_user_login(&state, &admin_cookie, "bob", "bobpassword1234").await;

    let local_path = tmp.path().join("workspaces").join("evict-me");
    std::fs::create_dir_all(&local_path).unwrap();
    let local_path_str = local_path.to_string_lossy().into_owned();
    let r1 = seed_repository(&state, "evict-me", None, &local_path_str).await;

    let body = format!(r#"{{"repository_id":"{r1}"}}"#);
    post_repositories(&state, &admin_cookie, &body).await;
    post_repositories(&state, &bob_cookie, &body).await;

    // Admin force_purges.
    let status =
        delete_repository(&state, &admin_cookie, &r1, Some(r#"{"force_purge":true}"#)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Filesystem + DB row + every association is gone.
    assert!(!local_path.exists());
    let db = state.auth().db.as_ref().unwrap();
    let row = maestro_core::db::repositories::get(db.adapter(), &r1)
        .await
        .unwrap();
    assert!(row.is_none());
    let (_, bob_mine) = list_mine(&state, &bob_cookie).await;
    assert!(bob_mine.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn ac8_non_admin_force_purge_returns_403() {
    let (state, _tmp) = test_state_isolated();
    let admin_cookie = register_admin(&state).await;
    let alice_cookie = create_user_login(&state, &admin_cookie, "alice", "alicepassword1234").await;

    let r1 = seed_repository(&state, "x", None, "/workspaces/x").await;
    let body = format!(r#"{{"repository_id":"{r1}"}}"#);
    post_repositories(&state, &alice_cookie, &body).await;

    let status =
        delete_repository(&state, &alice_cookie, &r1, Some(r#"{"force_purge":true}"#)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// ---------------------------------------------------------------------------
// User delete cascades to user_repositories (DB-level).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac11_user_delete_cascades_to_associations() {
    let (state, _tmp) = test_state_isolated();
    let admin_cookie = register_admin(&state).await;
    // Add a second admin so the "last admin" guard doesn't block the delete.
    let _co = create_user_login(&state, &admin_cookie, "co-admin", "coadminpw1234").await;
    let bob_cookie = create_user_login(&state, &admin_cookie, "bob", "bobpassword1234").await;

    let r1 = seed_repository(&state, "rcasc", None, "/workspaces/rcasc").await;
    let body = format!(r#"{{"repository_id":"{r1}"}}"#);
    post_repositories(&state, &bob_cookie, &body).await;

    // Delete bob via the admin endpoint.
    let db = state.auth().db.as_ref().unwrap().clone();
    let bob = maestro_core::db::users::get_user_by_username(db.adapter(), "bob")
        .await
        .unwrap()
        .unwrap();
    maestro_core::db::users::delete_user(db.adapter(), &bob.id)
        .await
        .unwrap();

    // The repo row survives but its association is gone.
    let assoc_count: i64 = db
        .adapter()
        .query_one(
            "SELECT COUNT(*) FROM user_repositories WHERE repository_id = ?",
            vec![maestro_core::db::DbValue::Text(r1.clone())],
        )
        .await
        .unwrap()
        .get_i64(0)
        .unwrap();
    assert_eq!(assoc_count, 0);
}

// ---------------------------------------------------------------------------
// Active workflow blocks delete.
// ---------------------------------------------------------------------------

/// Fetch the user_id of an existing username, directly from the DB. Used by
/// the active-workflow tests to seed workflow snapshots with realistic user_ids.
async fn user_id_for(state: &AppState, username: &str) -> String {
    let db = state.auth().db.as_ref().expect("db required").clone();
    db.adapter()
        .query_one(
            "SELECT id FROM users WHERE username = ?",
            vec![maestro_core::db::DbValue::Text(username.to_string())],
        )
        .await
        .expect("user must exist")
        .get_text(0)
        .expect("id text")
}

/// The caller's OWN active workflow on a repo blocks the caller's
/// delete of that repo (the workflow's worktree would orphan).
#[tokio::test]
async fn ac16_callers_active_workflow_blocks_delete() {
    use chrono::Utc;
    use maestro_core::workflow::snapshot::{
        PersistedWorkflowRecord, SNAPSHOT_FILENAME, SNAPSHOT_VERSION, WorkflowSnapshotFile,
    };
    use maestro_core::workflow::state::WorkflowState;

    let (state, tmp) = test_state_isolated();
    let cookie = register_admin(&state).await;
    let admin_uid = user_id_for(&state, "admin").await;

    let prev = std::env::var("MAESTRO_DATA_DIR").ok();
    // SAFETY: tests are single-threaded inside this function; we restore the
    // env at the end. The wider process may have other env access, but
    // `repository_has_active_workflow` reads it lazily and we explicitly
    // reset it after.
    unsafe {
        std::env::set_var("MAESTRO_DATA_DIR", tmp.path());
    }

    let r1 = seed_repository(&state, "active-wf-repo", None, "/workspaces/active-wf-repo").await;
    let body = format!(r#"{{"repository_id":"{r1}"}}"#);
    post_repositories(&state, &cookie, &body).await;

    // Drop an active workflow snapshot owned by the CALLER. Their own active
    // workflow must block their own delete (worktree would orphan).
    let snap_dir = tmp.path().join("workspaces").join("active-wf-repo");
    std::fs::create_dir_all(&snap_dir).unwrap();
    let snap_file = snap_dir.join(SNAPSHOT_FILENAME);
    let snap = WorkflowSnapshotFile {
        version: SNAPSHOT_VERSION,
        workflows: vec![PersistedWorkflowRecord {
            id: "wf-1".to_string(),
            ticket_key: "ACTIVE-1".to_string(),
            ticket_summary: "active".to_string(),
            ticket_description: String::new(),
            ticket_type: String::new(),
            state: WorkflowState::Pending,
            started_at: Utc::now(),
            updated_at: Utc::now(),
            steps_log: Vec::new(),
            branch_name: String::new(),
            worktree_path: None,
            pr_url: None,
            pr_merged: false,
            terminal_lines: Vec::new(),
            current_step_label: None,
            started_manually: true,
            jira_available: false,
            last_session_id: None,
            description_session_id: None,
            ticketing_system: maestro_core::config::TicketingSystem::None,
            ticket_url: None,
            driver_started: false,
            workflow_def_runs: std::collections::HashMap::new(),
            worktree_bootstrapped: false,
            workspace_name: "active-wf-repo".to_string(),
            repository_id: Some(r1.clone()),
            user_id: Some(admin_uid.clone()),
            auth_pin: None,
        }],
    };
    std::fs::write(&snap_file, serde_json::to_string(&snap).unwrap()).unwrap();

    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/repositories/{r1}"))
                .header("Origin", TEST_ORIGIN)
                .header("Cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["blocking_workflows"].is_array());
    let blockers = json["blocking_workflows"].as_array().unwrap();
    assert_eq!(blockers.len(), 1);
    assert_eq!(blockers[0]["ticket_key"], "ACTIVE-1");
    assert_eq!(blockers[0]["user_id"], admin_uid);

    // Restore env.
    unsafe {
        match prev {
            Some(v) => std::env::set_var("MAESTRO_DATA_DIR", v),
            None => std::env::remove_var("MAESTRO_DATA_DIR"),
        }
    }
}

/// ANOTHER user's active workflow on a repo MUST NOT block the
/// caller's delete. The caller is just dropping their own association; the
/// other user keeps theirs and their worktree stays valid.
///
/// Fix for the bug surfaced in production: user `alex` (non-admin) tried to
/// remove `maestro-core` and was 409'd because admin had an active workflow
/// on the repo.
#[tokio::test]
async fn ac16_other_users_active_workflow_does_not_block_caller_delete() {
    use chrono::Utc;
    use maestro_core::workflow::snapshot::{
        PersistedWorkflowRecord, SNAPSHOT_FILENAME, SNAPSHOT_VERSION, WorkflowSnapshotFile,
    };
    use maestro_core::workflow::state::WorkflowState;

    let (state, tmp) = test_state_isolated();
    let admin_cookie = register_admin(&state).await;
    let admin_uid = user_id_for(&state, "admin").await;
    let alice_cookie = create_user_login(&state, &admin_cookie, "alice", "AlicePass1234").await;

    let prev = std::env::var("MAESTRO_DATA_DIR").ok();
    unsafe {
        std::env::set_var("MAESTRO_DATA_DIR", tmp.path());
    }

    let r1 = seed_repository(&state, "shared-repo", None, "/workspaces/shared-repo").await;
    // Both users add the repo.
    let body = format!(r#"{{"repository_id":"{r1}"}}"#);
    post_repositories(&state, &admin_cookie, &body).await;
    post_repositories(&state, &alice_cookie, &body).await;

    // Drop an active workflow snapshot OWNED BY ADMIN (not alice).
    let snap_dir = tmp.path().join("workspaces").join("shared-repo");
    std::fs::create_dir_all(&snap_dir).unwrap();
    let snap_file = snap_dir.join(SNAPSHOT_FILENAME);
    let snap = WorkflowSnapshotFile {
        version: SNAPSHOT_VERSION,
        workflows: vec![PersistedWorkflowRecord {
            id: "wf-1".to_string(),
            ticket_key: "GH-65".to_string(),
            ticket_summary: "admin's active workflow".to_string(),
            ticket_description: String::new(),
            ticket_type: String::new(),
            state: WorkflowState::Pending,
            started_at: Utc::now(),
            updated_at: Utc::now(),
            steps_log: Vec::new(),
            branch_name: String::new(),
            worktree_path: None,
            pr_url: None,
            pr_merged: false,
            terminal_lines: Vec::new(),
            current_step_label: None,
            started_manually: true,
            jira_available: false,
            last_session_id: None,
            description_session_id: None,
            ticketing_system: maestro_core::config::TicketingSystem::None,
            ticket_url: None,
            driver_started: false,
            workflow_def_runs: std::collections::HashMap::new(),
            worktree_bootstrapped: false,
            workspace_name: "shared-repo".to_string(),
            repository_id: Some(r1.clone()),
            user_id: Some(admin_uid.clone()),
            auth_pin: None,
        }],
    };
    std::fs::write(&snap_file, serde_json::to_string(&snap).unwrap()).unwrap();

    // Alice removes her association — admin's workflow must NOT block her.
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/repositories/{r1}"))
                .header("Origin", TEST_ORIGIN)
                .header("Cookie", &alice_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NO_CONTENT,
        "alice should be allowed to remove her association even though admin has an active workflow on the repo"
    );

    // Restore env.
    unsafe {
        match prev {
            Some(v) => std::env::set_var("MAESTRO_DATA_DIR", v),
            None => std::env::remove_var("MAESTRO_DATA_DIR"),
        }
    }
}

// ---------------------------------------------------------------------------
// URL validation.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn post_with_no_body_fields_returns_400() {
    let (state, _tmp) = test_state_isolated();
    let cookie = register_admin(&state).await;
    let (status, _) = post_repositories(&state, &cookie, "{}").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn post_with_both_body_fields_returns_400() {
    let (state, _tmp) = test_state_isolated();
    let cookie = register_admin(&state).await;
    let (status, _) = post_repositories(
        &state,
        &cookie,
        r#"{"repository_id":"x","repo_url":"https://github.com/a/b"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn invalid_repo_urls_are_rejected_with_400() {
    let (state, _tmp) = test_state_isolated();
    let cookie = register_admin(&state).await;

    let bad_cases: Vec<(&str, &str)> = vec![
        (
            "credentials",
            r#"{"repo_url":"https://user:pass@github.com/owner/repo"}"#,
        ),
        (
            "query_string",
            r#"{"repo_url":"https://github.com/owner/repo?ref=main"}"#,
        ),
        (
            "fragment",
            r#"{"repo_url":"https://github.com/owner/repo#x"}"#,
        ),
        (
            "path_traversal",
            r#"{"repo_url":"https://github.com/owner/.."}"#,
        ),
        (
            "non_github",
            r#"{"repo_url":"https://gitlab.com/owner/repo"}"#,
        ),
        (
            "extra_segments",
            r#"{"repo_url":"https://github.com/owner/repo/tree/main"}"#,
        ),
        (
            "missing_repo",
            r#"{"repo_url":"https://github.com/owner/"}"#,
        ),
        (
            "invalid_char",
            r#"{"repo_url":"https://github.com/owner!/repo"}"#,
        ),
        (
            "http_scheme",
            r#"{"repo_url":"http://github.com/owner/repo"}"#,
        ),
    ];

    for (label, body) in bad_cases {
        let (status, _) = post_repositories(&state, &cookie, body).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "case {label} must be rejected: {body}"
        );
    }
}

#[tokio::test]
async fn oversize_repo_url_rejected_with_400() {
    let (state, _tmp) = test_state_isolated();
    let cookie = register_admin(&state).await;
    let huge = format!(
        r#"{{"repo_url":"https://github.com/owner/{}"}}"#,
        "a".repeat(3000)
    );
    let (status, _) = post_repositories(&state, &cookie, &huge).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn ac5_repo_url_already_known_short_circuits_clone_with_200() {
    // When the `repo_url` is already in `repositories`, the POST handler
    // associates the caller (idempotent) and returns 200 — no clone runs.
    // We can drive this without a real `git` because the existing-row
    // branch executes before `do_clone`.
    let (state, _tmp) = test_state_isolated();
    let cookie = register_admin(&state).await;

    let url = "https://github.com/owner/precloned";
    let _r1 = seed_repository(&state, "precloned", Some(url), "/workspaces/precloned").await;

    let body = format!(r#"{{"repo_url":"{url}"}}"#);
    let (status, json) = post_repositories(&state, &cookie, &body).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["name"], "precloned");

    // My list now contains it.
    let (_, mine) = list_mine(&state, &cookie).await;
    assert_eq!(mine.as_array().unwrap().len(), 1);
}

// ---------------------------------------------------------------------------
// Concurrency — lock test via state.engine().clone_in_progress.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ac15_clone_in_progress_returns_409_when_locked() {
    let (state, _tmp) = test_state_isolated();
    let cookie = register_admin(&state).await;
    // Force the lock to "busy" so the dispatch fails fast without shelling out.
    state
        .engine()
        .clone_in_progress
        .store(true, std::sync::atomic::Ordering::Release);

    // A repo_url not yet in DB → would attempt to clone but bail on the lock.
    let (status, body) = post_repositories(
        &state,
        &cookie,
        r#"{"repo_url":"https://github.com/owner/never-cloned"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let text = body.as_str().unwrap_or_default();
    assert!(text.contains("clone already in progress"), "got: {text}");
}

// ---------------------------------------------------------------------------
// Response-shape sanity — the dto contract.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mine_dto_includes_added_at_and_co_users_count() {
    let (state, _tmp) = test_state_isolated();
    let admin_cookie = register_admin(&state).await;
    let bob_cookie = create_user_login(&state, &admin_cookie, "bob", "bobpassword1234").await;

    let r1 = seed_repository(&state, "with-co", None, "/workspaces/with-co").await;
    let body = format!(r#"{{"repository_id":"{r1}"}}"#);
    post_repositories(&state, &admin_cookie, &body).await;
    post_repositories(&state, &bob_cookie, &body).await;

    let (_, mine) = list_mine(&state, &admin_cookie).await;
    let row = &mine.as_array().unwrap()[0];
    assert!(row["added_at"].is_number());
    assert_eq!(row["co_users_count"], 1, "bob is the only co-user");

    let (_, avail_admin) = list_available(&state, &admin_cookie).await;
    // For admin, available should NOT include the repo we just added.
    assert!(avail_admin.as_array().unwrap().is_empty());

    // Available rows must NOT include added_at.
    let _bob_id = seed_repository(&state, "extra", None, "/workspaces/extra").await;
    let (_, avail_again) = list_available(&state, &admin_cookie).await;
    let extra_row = avail_again.as_array().unwrap()[0].clone();
    assert!(extra_row["added_at"].is_null());
}
