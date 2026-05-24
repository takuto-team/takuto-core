// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

// Copyright (C) 2026 Alexandre Obellianne
//
// Phase 0 integration tests for `GET /api/onboarding/status` and the three new
// `GET /api/auth/status` fields (provider_selected, github_mode, degraded).
// Source-of-truth contract: tmp/multi-agents/04_architecture.md §1.3.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tokio::sync::RwLock;
use tower::ServiceExt;

use maestro_core::actions::dry_run::DryRunActions;
use maestro_core::config::{Config, TicketingSystem};
use maestro_core::docker_hooks::{StructuredWarning, SystemStatus};
use maestro_core::workflow::engine::WorkflowEngine;
use maestro_web::server::build_router;
use maestro_web::state::AppState;
use maestro_web::test_helpers::test_state_with_db;

/// Build an `AppState` with `db: None` — used by T-BOOT-010 to verify the
/// boot-without-database degraded mode. Mirrors `test_state_with_db_instance`
/// but skips the SQLite handle.
fn test_state_no_db() -> AppState {
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
        std::env::temp_dir(),
    ));
    use maestro_web::state::{AuthState, ConfigState, EditorState, EngineState, RunCommandState};
    AppState::new(
        EngineState {
            engine,
            polling_paused: Arc::new(AtomicBool::new(false)),
            clone_in_progress: Arc::new(AtomicBool::new(false)),
            system_status: Arc::new(RwLock::new(SystemStatus::default())),
        },
        AuthState {
            db: None,
            gh_client: Arc::new(maestro_core::auth::RealGhClient::new()),
            git_auth_resolver: None,
        },
        ConfigState {
            config,
            config_path: std::env::temp_dir().join("config.toml"),
            config_writer: None,
            ticketing_system: TicketingSystem::None,
            jira_available,
            preflight_error: None,
        },
        EditorState {
            editor_scanners: Arc::new(RwLock::new(HashMap::new())),
            dynamic_forwards: Arc::new(RwLock::new(HashMap::new())),
            terminal_ports: Arc::new(RwLock::new(HashMap::new())),
            editor_bundles: Arc::new(RwLock::new(HashMap::new())),
            path_token_registry: maestro_web::session_registry::PathTokenRegistry::new(),
        },
        RunCommandState {
            run_commands: Arc::new(RwLock::new(HashMap::new())),
            run_command_bundles: Arc::new(RwLock::new(HashMap::new())),
        },
    )
}

/// Public access: no session cookie required to read `/api/onboarding/status`.
#[tokio::test]
async fn onboarding_status_is_public_and_returns_system_status_shape() {
    let state = test_state_with_db();
    let app = build_router(state);

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
    // Parse via the public type so the test breaks loudly if the wire shape
    // diverges from the documented contract.
    let parsed: SystemStatus = serde_json::from_slice(&body).expect("SystemStatus JSON shape");

    // Default fixture: no warnings, conservative defaults.
    assert!(parsed.config_toml_ok);
    assert!(!parsed.has_critical());
    assert_eq!(parsed.provider.selected, "claude");
    assert_eq!(parsed.github.mode, "missing");
    assert_eq!(parsed.ticketing.system, "none");

    // Also ensure raw JSON keys are present (catches accidental field rename).
    let raw: serde_json::Value = serde_json::from_slice(&body).unwrap();
    for key in &[
        "config_toml_ok",
        "github",
        "provider",
        "ticketing",
        "per_user_required",
        "warnings",
    ] {
        assert!(raw.get(*key).is_some(), "missing top-level field `{key}`");
    }
    for key in &["mode", "app_configured", "app_id", "app_name"] {
        assert!(raw["github"].get(*key).is_some(), "missing github.{key}");
    }
    for key in &[
        "selected",
        "deployment_default_credential_present",
        "headless_capable",
        "custom_base_url",
    ] {
        assert!(raw["provider"].get(*key).is_some(), "missing provider.{key}");
    }
    for key in &["system", "acli_ok"] {
        assert!(raw["ticketing"].get(*key).is_some(), "missing ticketing.{key}");
    }
}

/// When a critical warning is present, `/api/auth/status` reports `degraded: true`.
#[tokio::test]
async fn auth_status_reports_degraded_when_critical_warning_present() {
    let state = test_state_with_db();
    {
        let mut s = state.engine.system_status.write().await;
        s.warnings.push(StructuredWarning {
            code: "claude_not_authenticated".into(),
            severity: "critical".into(),
            message: "test fixture".into(),
        });
        s.provider.selected = "claude".into();
        s.github.mode = "missing".into();
    }

    let app = build_router(state);
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

    assert_eq!(json["provider_selected"], "claude");
    assert_eq!(json["github_mode"], "missing");
    assert_eq!(
        json["degraded"], true,
        "degraded flag should reflect critical warnings"
    );
}

/// T-ONB-001 (Phase 1, P0): on the very first POST /api/auth/register (zero
/// users in the DB), the 201 response body includes `redirect_to: "/onboarding"`
/// so non-browser API consumers and the UI can both route the just-created
/// admin to the 4-step onboarding wizard without hard-coding the path.
#[tokio::test]
async fn register_first_admin_response_includes_redirect_to_onboarding() {
    let state = test_state_with_db(); // DB present, zero users.
    let app = build_router(state);

    let resp = app
        .oneshot(
            Request::post("/api/auth/register")
                .header("Content-Type", "application/json")
                .header("Origin", "http://localhost:8080")
                .body(Body::from(
                    r#"{"username":"admin","password":"testpassword1234"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["redirect_to"], "/onboarding");
    // Other fields stay intact — adding the key must not regress Phase 0.
    assert_eq!(json["username"], "admin");
    assert_eq!(json["role"], "admin");
    assert!(json["user_id"].is_string());
    assert!(json["recovery_codes"].is_array());
}

/// T-BOOT-001 (P0): when every check has failed and `system_status.warnings`
/// is populated, `/api/health` still returns 200 (the server stays alive in
/// degraded mode) and `/api/onboarding/status` returns the populated status
/// with a non-empty `warnings` array.
#[tokio::test]
async fn health_ok_and_onboarding_exposes_warnings_when_everything_broken() {
    let state = test_state_with_db();
    {
        let mut s = state.engine.system_status.write().await;
        s.warnings.push(StructuredWarning {
            code: "claude_not_authenticated".into(),
            severity: "critical".into(),
            message: "claude broken".into(),
        });
        s.warnings.push(StructuredWarning {
            code: "gh_auth_missing".into(),
            severity: "critical".into(),
            message: "gh broken".into(),
        });
        s.warnings.push(StructuredWarning {
            code: "acli_not_authenticated".into(),
            severity: "warning".into(),
            message: "acli broken".into(),
        });
    }

    // /api/health must stay 200 — the server boots into degraded mode rather
    // than crashing or returning 5xx.
    let app = build_router(state.clone());
    let health_resp = app
        .oneshot(Request::get("/api/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(health_resp.status(), StatusCode::OK);

    // /api/onboarding/status returns the populated SystemStatus.
    let app = build_router(state);
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
    let parsed: SystemStatus = serde_json::from_slice(&body).expect("SystemStatus JSON shape");
    assert!(!parsed.warnings.is_empty(), "warnings should be exposed");
    assert!(parsed.has_critical(), "has_critical reflects warning set");
    let codes: Vec<&str> = parsed.warnings.iter().map(|w| w.code.as_str()).collect();
    assert!(codes.contains(&"claude_not_authenticated"));
    assert!(codes.contains(&"gh_auth_missing"));
    assert!(codes.contains(&"acli_not_authenticated"));
}

/// T-BOOT-002 (P0): `/api/auth/status` reports `setup_required: true`,
/// `provider_selected`, `github_mode: "missing"`, and `degraded: true`
/// simultaneously when the system has critical warnings and no users have
/// been registered yet.
#[tokio::test]
async fn auth_status_setup_required_and_degraded_when_no_users_and_critical() {
    let state = test_state_with_db(); // DB present, zero users registered.
    {
        let mut s = state.engine.system_status.write().await;
        s.warnings.push(StructuredWarning {
            code: "claude_not_authenticated".into(),
            severity: "critical".into(),
            message: "test fixture".into(),
        });
        s.provider.selected = "claude".into();
        s.github.mode = "missing".into();
    }

    let app = build_router(state);
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

    assert_eq!(json["setup_required"], true);
    assert_eq!(json["provider_selected"], "claude");
    assert_eq!(json["github_mode"], "missing");
    assert_eq!(json["degraded"], true);
}

/// T-BOOT-010 (P0): when `AppState::db` is `None`, the three public endpoints
/// (`/api/health`, `/api/auth/status`, `/api/onboarding/status`) all succeed,
/// while a representative protected endpoint (`/api/workflows`) returns 401.
/// This proves the server boots into degraded mode when the data directory is
/// unavailable and never serves protected data without a DB-backed session.
#[tokio::test]
async fn boots_in_degraded_mode_when_database_is_unavailable() {
    let state = test_state_no_db();

    // /api/health — 200.
    let app = build_router(state.clone());
    let resp = app
        .oneshot(Request::get("/api/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // /api/auth/status — 200, setup_required=true because no DB means no users.
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
    assert_eq!(json["setup_required"], true);

    // /api/onboarding/status — 200.
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

    // Protected endpoint — 401.
    let app = build_router(state);
    let resp = app
        .oneshot(Request::get("/api/workflows").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// `/api/auth/status` returns the three new mirrored fields with the right
/// values even on a clean boot.
#[tokio::test]
async fn auth_status_includes_phase0_mirrored_fields_on_clean_boot() {
    let state = test_state_with_db();
    {
        let mut s = state.engine.system_status.write().await;
        s.provider.selected = "cursor".into();
        s.github.mode = "app".into();
    }
    // No warnings → degraded must be false.

    let app = build_router(state);
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

    assert_eq!(json["provider_selected"], "cursor");
    assert_eq!(json["github_mode"], "app");
    assert_eq!(json["degraded"], false);
}

// ────────────────────────────────────────────────────────────────────────
// T-ONB-FILTER-* (task #30): per-request warning filter
//
// `GET /api/onboarding/status` filters provider/gh warnings against the
// calling user's stored credentials. Public callers (no cookie) still see
// the raw warnings.
// ────────────────────────────────────────────────────────────────────────

/// Seed an authenticated user via the existing helper and return their
/// session cookie + the resolved `user_id` (looked up by username).
async fn register_admin_and_get_id(state: &AppState) -> (String, String) {
    let cookie = maestro_web::test_helpers::register_and_login(state).await;
    let db = state.auth.db.clone().expect("test state must have a DB");
    let user_id = tokio::task::spawn_blocking(move || {
        let conn = db.conn().blocking_lock();
        maestro_core::db::users::get_user_by_username(&conn, "admin")
            .expect("db query")
            .expect("admin must exist")
            .id
    })
    .await
    .expect("join");
    (cookie, user_id)
}

/// Insert a provider credential row directly. Uses the test DB's master
/// key so the `seal()` envelope matches what production would produce.
async fn seed_provider_credential(state: &AppState, user_id: &str, provider: &str) {
    let db = state.auth.db.clone().expect("test DB");
    let user_id = user_id.to_string();
    let provider = provider.to_string();
    tokio::task::spawn_blocking(move || {
        let mk = db
            .master_key()
            .expect("test DB must have master key")
            .key
            .clone();
        let sealed = maestro_core::auth::seal(&mk, b"sk-test-token").unwrap();
        let conn = db.conn().blocking_lock();
        maestro_core::db::provider_credentials::upsert(
            &conn,
            &user_id,
            &provider,
            maestro_core::db::provider_credentials::ProviderCredentialKind::ApiKey,
            &sealed,
            "{}",
        )
        .expect("seed provider credential");
    })
    .await
    .expect("join");
}

/// Insert a GitHub PAT row directly via the DB helper.
async fn seed_github_credential(state: &AppState, user_id: &str) {
    let db = state.auth.db.clone().expect("test DB");
    let user_id = user_id.to_string();
    tokio::task::spawn_blocking(move || {
        let mk = db.master_key().expect("test DB master key").key.clone();
        let sealed = maestro_core::auth::seal(&mk, b"ghp_test_pat").unwrap();
        let conn = db.conn().blocking_lock();
        maestro_core::db::github_credentials::upsert(
            &conn,
            &user_id,
            &sealed,
            "alice",
            "[\"repo\"]",
            true,
        )
        .expect("seed github credential");
    })
    .await
    .expect("join");
}

/// Push the standard fixture into `system_status.warnings`: the active
/// provider's `_not_authenticated` warning, `gh_auth_missing`, and a
/// platform-level `master_key_unavailable` (which must always survive
/// filtering).
async fn seed_warnings(
    state: &AppState,
    provider_warning_code: &str,
    include_gh: bool,
    include_master_key: bool,
) {
    let mut s = state.engine.system_status.write().await;
    s.warnings.clear();
    s.warnings.push(StructuredWarning {
        code: provider_warning_code.into(),
        severity: "critical".into(),
        message: "fixture".into(),
    });
    if include_gh {
        s.warnings.push(StructuredWarning {
            code: "gh_auth_missing".into(),
            severity: "critical".into(),
            message: "fixture".into(),
        });
    }
    if include_master_key {
        s.warnings.push(StructuredWarning {
            code: "master_key_unavailable".into(),
            severity: "critical".into(),
            message: "fixture".into(),
        });
    }
}

async fn warnings_for_request(state: AppState, cookie: Option<&str>) -> Vec<String> {
    let mut req = Request::get("/api/onboarding/status");
    if let Some(c) = cookie {
        req = req.header("Cookie", c);
    }
    let app = build_router(state);
    let resp = app.oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    v["warnings"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|w| w["code"].as_str().unwrap_or("").to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// T-ONB-FILTER-001 (P0): authenticated user with active claude credential
/// + `provider = "claude"` in config → response warnings do NOT contain
/// `claude_not_authenticated`. (Platform/admin warnings survive.)
#[tokio::test]
async fn t_onb_filter_001_active_provider_warning_dropped_for_user_with_credential() {
    let state = test_state_with_db();
    let (cookie, user_id) = register_admin_and_get_id(&state).await;
    // Default config: provider = "claude". Seed a claude credential.
    seed_provider_credential(&state, &user_id, "claude").await;
    seed_warnings(&state, "claude_not_authenticated", false, true).await;
    {
        let mut s = state.engine.system_status.write().await;
        s.provider.selected = "claude".into();
    }

    let codes = warnings_for_request(state, Some(&cookie)).await;
    assert!(
        !codes.iter().any(|c| c == "claude_not_authenticated"),
        "active provider warning must be filtered for user with credential; got {codes:?}"
    );
    assert!(
        codes.iter().any(|c| c == "master_key_unavailable"),
        "platform warning must survive; got {codes:?}"
    );
}

/// T-ONB-FILTER-002 (P0): active provider Cursor, user has Claude cred but
/// not Cursor → `cursor_not_authenticated` stays in the response.
#[tokio::test]
async fn t_onb_filter_002_mismatched_credential_does_not_filter_active_warning() {
    let state = test_state_with_db();
    let (cookie, user_id) = register_admin_and_get_id(&state).await;
    // Seed CLAUDE credential, set active provider to CURSOR.
    seed_provider_credential(&state, &user_id, "claude").await;
    {
        let mut cfg = state.config.config.write().await;
        cfg.agent.provider = maestro_core::config::AiAgentProvider::Cursor;
    }
    seed_warnings(&state, "cursor_not_authenticated", false, false).await;
    {
        let mut s = state.engine.system_status.write().await;
        s.provider.selected = "cursor".into();
    }

    let codes = warnings_for_request(state, Some(&cookie)).await;
    assert!(
        codes.iter().any(|c| c == "cursor_not_authenticated"),
        "active provider warning must stay when user has no matching credential; got {codes:?}"
    );
}

/// T-ONB-FILTER-003 (P0): authenticated user without any credentials →
/// active provider warning still present.
#[tokio::test]
async fn t_onb_filter_003_no_credential_user_still_sees_active_warning() {
    let state = test_state_with_db();
    let (cookie, _user_id) = register_admin_and_get_id(&state).await;
    seed_warnings(&state, "claude_not_authenticated", false, false).await;
    {
        let mut s = state.engine.system_status.write().await;
        s.provider.selected = "claude".into();
    }

    let codes = warnings_for_request(state, Some(&cookie)).await;
    assert!(
        codes.iter().any(|c| c == "claude_not_authenticated"),
        "active provider warning must stay for user with no credential; got {codes:?}"
    );
}

/// T-ONB-FILTER-004 (P0): GitHub App configured → `gh_auth_missing`
/// dropped regardless of user PAT.
#[tokio::test]
async fn t_onb_filter_004_gh_auth_missing_dropped_when_app_configured() {
    let state = test_state_with_db();
    let (cookie, _user_id) = register_admin_and_get_id(&state).await;
    {
        let mut cfg = state.config.config.write().await;
        cfg.github.app_id = 12345;
        cfg.github.app_installation_id = 67890;
        cfg.github.app_private_key = "FAKE_PEM_BODY".into();
        assert!(cfg.github.is_configured(), "test setup: app must be configured");
    }
    seed_warnings(&state, "claude_not_authenticated", true, false).await;

    let codes = warnings_for_request(state, Some(&cookie)).await;
    assert!(
        !codes.iter().any(|c| c == "gh_auth_missing"),
        "gh_auth_missing must be dropped when App is configured; got {codes:?}"
    );
}

/// T-ONB-FILTER-005 (P0): App NOT configured + user has a PAT row →
/// `gh_auth_missing` dropped.
#[tokio::test]
async fn t_onb_filter_005_gh_auth_missing_dropped_when_user_has_pat() {
    let state = test_state_with_db();
    let (cookie, user_id) = register_admin_and_get_id(&state).await;
    // App stays unconfigured by default. Seed user PAT.
    seed_github_credential(&state, &user_id).await;
    seed_warnings(&state, "claude_not_authenticated", true, false).await;

    let codes = warnings_for_request(state, Some(&cookie)).await;
    assert!(
        !codes.iter().any(|c| c == "gh_auth_missing"),
        "gh_auth_missing must be dropped when user has PAT; got {codes:?}"
    );
}

/// T-ONB-FILTER-006 (P0): App NOT configured + user has no PAT →
/// `gh_auth_missing` stays.
#[tokio::test]
async fn t_onb_filter_006_gh_auth_missing_kept_when_neither_app_nor_pat() {
    let state = test_state_with_db();
    let (cookie, _user_id) = register_admin_and_get_id(&state).await;
    seed_warnings(&state, "claude_not_authenticated", true, false).await;

    let codes = warnings_for_request(state, Some(&cookie)).await;
    assert!(
        codes.iter().any(|c| c == "gh_auth_missing"),
        "gh_auth_missing must stay when neither App nor user PAT; got {codes:?}"
    );
}

/// T-ONB-FILTER-007 (P0): unauthenticated request → raw warnings returned
/// (no filtering possible without a user).
#[tokio::test]
async fn t_onb_filter_007_unauthenticated_request_returns_raw_warnings() {
    let state = test_state_with_db();
    seed_warnings(&state, "claude_not_authenticated", true, true).await;
    {
        let mut s = state.engine.system_status.write().await;
        s.provider.selected = "claude".into();
    }

    let codes = warnings_for_request(state, None).await;
    assert!(
        codes.iter().any(|c| c == "claude_not_authenticated"),
        "unauthenticated request must keep raw warnings; got {codes:?}"
    );
    assert!(codes.iter().any(|c| c == "gh_auth_missing"));
    assert!(codes.iter().any(|c| c == "master_key_unavailable"));
}

/// T-ONB-FILTER-008 (P1): platform warnings (`master_key_unavailable`)
/// survive filtering for all users, including ones with full credentials.
#[tokio::test]
async fn t_onb_filter_008_platform_warning_survives_for_fully_set_up_user() {
    let state = test_state_with_db();
    let (cookie, user_id) = register_admin_and_get_id(&state).await;
    // Fully set up: provider cred + GH App configured + PAT seeded.
    seed_provider_credential(&state, &user_id, "claude").await;
    seed_github_credential(&state, &user_id).await;
    {
        let mut cfg = state.config.config.write().await;
        cfg.github.app_id = 1;
        cfg.github.app_installation_id = 1;
        cfg.github.app_private_key = "FAKE".into();
    }
    seed_warnings(&state, "claude_not_authenticated", true, true).await;

    let codes = warnings_for_request(state, Some(&cookie)).await;
    // User-filterable warnings dropped.
    assert!(!codes.iter().any(|c| c == "claude_not_authenticated"));
    assert!(!codes.iter().any(|c| c == "gh_auth_missing"));
    // Platform warning survives.
    assert!(
        codes.iter().any(|c| c == "master_key_unavailable"),
        "platform warning must survive for fully-set-up user; got {codes:?}"
    );
}
