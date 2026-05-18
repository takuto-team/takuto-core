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
    AppState {
        engine,
        config,
        db: None,
        polling_paused: Arc::new(AtomicBool::new(false)),
        jira_available,
        ticketing_system: TicketingSystem::None,
        editor_scanners: Arc::new(RwLock::new(HashMap::new())),
        dynamic_forwards: Arc::new(RwLock::new(HashMap::new())),
        terminal_ports: Arc::new(RwLock::new(HashMap::new())),
        run_commands: Arc::new(RwLock::new(HashMap::new())),
        preflight_error: None,
        system_status: SystemStatus::default(),
        config_path: std::env::temp_dir().join("config.toml"),
        config_writer: None,
        clone_in_progress: Arc::new(AtomicBool::new(false)),
        path_token_registry: maestro_web::session_registry::PathTokenRegistry::new(),
    }
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
    let mut state = test_state_with_db();
    state.system_status.warnings.push(StructuredWarning {
        code: "claude_not_authenticated".into(),
        severity: "critical".into(),
        message: "test fixture".into(),
    });
    state.system_status.provider.selected = "claude".into();
    state.system_status.github.mode = "missing".into();

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

/// T-BOOT-001 (P0): when every check has failed and `system_status.warnings`
/// is populated, `/api/health` still returns 200 (the server stays alive in
/// degraded mode) and `/api/onboarding/status` returns the populated status
/// with a non-empty `warnings` array.
#[tokio::test]
async fn health_ok_and_onboarding_exposes_warnings_when_everything_broken() {
    let mut state = test_state_with_db();
    state.system_status.warnings.push(StructuredWarning {
        code: "claude_not_authenticated".into(),
        severity: "critical".into(),
        message: "claude broken".into(),
    });
    state.system_status.warnings.push(StructuredWarning {
        code: "gh_auth_missing".into(),
        severity: "critical".into(),
        message: "gh broken".into(),
    });
    state.system_status.warnings.push(StructuredWarning {
        code: "acli_not_authenticated".into(),
        severity: "warning".into(),
        message: "acli broken".into(),
    });

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
    let mut state = test_state_with_db(); // DB present, zero users registered.
    state.system_status.warnings.push(StructuredWarning {
        code: "claude_not_authenticated".into(),
        severity: "critical".into(),
        message: "test fixture".into(),
    });
    state.system_status.provider.selected = "claude".into();
    state.system_status.github.mode = "missing".into();

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
    let mut state = test_state_with_db();
    state.system_status.provider.selected = "cursor".into();
    state.system_status.github.mode = "app".into();
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
