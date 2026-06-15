// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

// Test-only `std::env` mutation (unsafe in the 2024 edition); serialised within the test.
#![allow(unsafe_code)]

// Copyright (C) 2026 Alexandre Obellianne
//
// Integration tests for poller workflow ownership and one-shot orphan
// migration. These tests do not exercise the binary's startup directly; instead
// they reproduce the same sequence the binary does (open DB → restore snapshot →
// optionally migrate orphans → build router) so the migration helper's effect
// on the visible workflow list can be verified end-to-end.
//
// The two scenarios in the spec are bundled into one tokio test so that the
// process-wide `TAKUTO_DATA_DIR` env var (read by `resolve_snapshot_dir`) is
// changed sequentially. Running them in separate `#[tokio::test]` functions
// would race because cargo runs them on the same threadpool.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::Utc;
use http_body_util::BodyExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tokio::sync::RwLock;
use tower::ServiceExt;

use takuto_core::actions::dry_run::DryRunActions;
use takuto_core::config::{Config, TicketingSystem};
use takuto_core::db::Database;
use takuto_core::workflow::engine::WorkflowEngine;
use takuto_core::workflow::snapshot::{
    PersistedWorkflowRecord, SNAPSHOT_FILENAME, SNAPSHOT_VERSION, WorkflowSnapshotFile,
};
use takuto_core::workflow::state::WorkflowState;
use takuto_web::server::build_router;
use takuto_web::state::AppState;
use takuto_web::test_helpers::register_and_login;

/// Build a complete AppState backed by a real Database that lives under
/// `data_dir`. The engine's `restore_persisted_workflows` will read snapshots
/// from `{data_dir}/workspaces/*/workflow_snapshot.json` because
/// `TAKUTO_DATA_DIR` is set to `data_dir` for the duration of the test.
fn build_state(data_dir: &std::path::Path) -> AppState {
    let db = Database::open(data_dir, true).expect("open temp DB");
    let mut cfg = Config::default();
    // Point repo_path at the data dir so resolve_snapshot_dir falls back
    // there if TAKUTO_DATA_DIR somehow gets unset between Test A and Test B.
    cfg.git.repo_path = data_dir.to_string_lossy().into_owned();
    let config = Arc::new(RwLock::new(cfg));
    let actions: Arc<dyn takuto_core::actions::traits::ExternalActions> =
        Arc::new(DryRunActions::new("origin".to_string(), None));
    let jira_available = Arc::new(AtomicBool::new(false));
    let engine = Arc::new(WorkflowEngine::new(
        config.clone(),
        actions,
        1,
        jira_available.clone(),
        TicketingSystem::None,
        data_dir.join("workflows"),
    ));
    std::fs::create_dir_all(data_dir.join("workflows")).ok();
    let git_auth_resolver = Some(Arc::new(
        takuto_core::github::auth_resolver::GitAuthResolver::new(db.clone(), None),
    ));
    use takuto_web::state::{AuthState, ConfigState, EditorState, EngineState, RunCommandState};
    AppState::new(
        EngineState {
            engine,
            polling_paused: Arc::new(AtomicBool::new(false)),
            clone_in_progress: Arc::new(AtomicBool::new(false)),
            system_status: Arc::new(RwLock::new(
                takuto_core::docker_hooks::SystemStatus::default(),
            )),
        },
        AuthState {
            db: Some(db),
            gh_client: Arc::new(takuto_core::auth::RealGhClient::new()),
            git_auth_resolver,
            jira_http: Arc::new(takuto_core::jira::RealJiraHttp::new()),
        },
        ConfigState {
            config,
            config_path: data_dir.join("config.toml"),
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
            path_token_registry: takuto_web::session_registry::PathTokenRegistry::new(),
        },
        RunCommandState {
            run_commands: Arc::new(RwLock::new(std::collections::HashMap::new())),
            run_command_bundles: Arc::new(RwLock::new(std::collections::HashMap::new())),
        },
    )
}

/// Pre-seed a workspace snapshot at `{data_dir}/workspaces/{ws}/workflow_snapshot.json`
/// containing one orphan workflow (`user_id: None`) in `Done` state. `Done` is
/// chosen so the persistence layer does not spawn a driver on restore.
fn seed_orphan_snapshot(data_dir: &std::path::Path, ws_name: &str, ticket_key: &str) {
    let ws_dir = data_dir.join("workspaces").join(ws_name);
    std::fs::create_dir_all(&ws_dir).expect("create workspace dir");

    let rec = PersistedWorkflowRecord {
        id: uuid::Uuid::new_v4().to_string(),
        ticket_key: ticket_key.to_string(),
        ticket_summary: "Orphan from before multi-user".into(),
        ticket_description: String::new(),
        ticket_type: "Task".into(),
        state: WorkflowState::Done,
        started_at: Utc::now(),
        updated_at: Utc::now(),
        steps_log: Vec::new(),
        branch_name: String::new(),
        worktree_path: None,
        pr_url: None,
        pr_merged: false,
        terminal_lines: Vec::new(),
        current_step_label: None,
        started_manually: false,
        jira_available: false,
        last_session_id: None,
        description_session_id: None,
        ticketing_system: TicketingSystem::None,
        ticket_url: None,
        driver_started: false,
        workflow_def_runs: std::collections::HashMap::new(),
        worktree_bootstrapped: false,
        workspace_name: ws_name.to_string(),
        repository_id: None,
        user_id: None,
        auth_pin: None,
    };

    let file = WorkflowSnapshotFile {
        version: SNAPSHOT_VERSION,
        workflows: vec![rec],
    };
    let path = ws_dir.join(SNAPSHOT_FILENAME);
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&file).expect("serialize snapshot"),
    )
    .expect("write snapshot");
}

/// Fresh, unique data dir for one phase of the test.
fn fresh_data_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "takuto-poller-owner-{}-{}",
        label,
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).expect("create temp data dir");
    dir
}

#[tokio::test]
async fn orphan_migration_e2e() {
    // ── Case A: migration enabled — workflow is adopted and becomes visible ──
    {
        let data_dir = fresh_data_dir("case-a");
        // Workspace name MUST match the active workspace the engine will use
        // (which is the basename of `git.repo_path` set in `build_state`).
        let ws_name = data_dir
            .file_name()
            .and_then(|n| n.to_str())
            .expect("data_dir has utf-8 file name");
        seed_orphan_snapshot(&data_dir, ws_name, "POLLED-1");

        // SAFETY: tests in this file are bundled into a single tokio test, so
        // setting a process-global env var is safe.
        unsafe {
            std::env::set_var("TAKUTO_DATA_DIR", &data_dir);
        }

        let state = build_state(&data_dir);
        let admin_cookie = register_and_login(&state).await;

        // Look up the admin's user_id — this is what the migration should
        // assign to the orphan workflow so the admin sees it in their list.
        let admin_id = {
            let db = state.auth().db.as_ref().unwrap().clone();
            takuto_core::db::users::get_user_by_username(db.adapter(), "admin")
                .await
                .unwrap()
                .unwrap()
                .id
        };

        let restored = state
            .engine()
            .engine
            .restore_persisted_workflows()
            .await
            .expect("restore");
        assert_eq!(restored, 1, "expected to restore one orphan workflow");

        // Sanity: before migration, the workflow is orphaned and invisible.
        {
            let wf_arc = state.engine().engine.workflows_arc();
            let map = wf_arc.read().await;
            let w = map.get("POLLED-1").expect("workflow present after restore");
            assert!(
                w.user_id.is_none(),
                "pre-migration: orphan workflow should have user_id = None"
            );
        }

        // The workflow list endpoint additionally gates visibility on the
        // caller's `user_repositories` set. Seed a `repositories` row matching
        // the workflow's `workspace_name` and associate it with the admin so
        // the post-migration assertion still observes the workflow. (Startup
        // reconciliation does this for real boots; this test bypasses
        // startup, so we do it inline.)
        {
            let db = state.auth().db.as_ref().unwrap();
            let adapter = db.adapter();
            let repo_id = takuto_core::db::repositories::upsert(
                adapter,
                ws_name,
                None,
                &format!("/workspaces/{ws_name}"),
                "main",
                None,
            )
            .await
            .expect("upsert repository");
            takuto_core::db::repositories::add_for_user(adapter, &admin_id, &repo_id)
                .await
                .expect("add_for_user");
        }

        // Run the migration helper using the admin's user_id as owner.
        let migrated = state
            .engine()
            .engine
            .migrate_orphan_workflows_to_owner(&admin_id)
            .await;
        assert_eq!(migrated, 1, "exactly one workflow should migrate");

        // After migration the workflow carries the admin's user_id.
        {
            let wf_arc = state.engine().engine.workflows_arc();
            let map = wf_arc.read().await;
            let w = map.get("POLLED-1").expect("workflow still present");
            assert_eq!(
                w.user_id.as_deref(),
                Some(admin_id.as_str()),
                "migration should have set user_id to the admin's id"
            );
        }

        // And it is visible via GET /api/workflows for the admin.
        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::get("/api/workflows")
                    .header("Cookie", &admin_cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let workflows = json.as_array().expect("list response");
        let keys: Vec<&str> = workflows
            .iter()
            .filter_map(|w| w.get("ticket_key").and_then(|v| v.as_str()))
            .collect();
        assert!(
            keys.contains(&"POLLED-1"),
            "migrated workflow should appear in GET /api/workflows; got keys: {keys:?}"
        );

        let _ = std::fs::remove_dir_all(&data_dir);
    }

    // ── Case B: migration disabled — workflow stays orphaned and invisible ──
    {
        let data_dir = fresh_data_dir("case-b");
        let ws_name = data_dir
            .file_name()
            .and_then(|n| n.to_str())
            .expect("data_dir has utf-8 file name");
        seed_orphan_snapshot(&data_dir, ws_name, "POLLED-2");

        unsafe {
            std::env::set_var("TAKUTO_DATA_DIR", &data_dir);
        }

        let state = build_state(&data_dir);
        let admin_cookie = register_and_login(&state).await;

        let restored = state
            .engine()
            .engine
            .restore_persisted_workflows()
            .await
            .expect("restore");
        assert_eq!(restored, 1, "expected to restore one orphan workflow");

        // Migration disabled: skip the migration helper entirely.

        // Orphan should still have user_id = None.
        {
            let wf_arc = state.engine().engine.workflows_arc();
            let map = wf_arc.read().await;
            let w = map.get("POLLED-2").expect("workflow present");
            assert!(
                w.user_id.is_none(),
                "without migration, workflow must remain unowned"
            );
        }

        // And it should NOT appear in GET /api/workflows for the admin
        // (per-user filter strips workflows whose user_id != caller).
        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::get("/api/workflows")
                    .header("Cookie", &admin_cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let workflows = json.as_array().expect("list response");
        let keys: Vec<&str> = workflows
            .iter()
            .filter_map(|w| w.get("ticket_key").and_then(|v| v.as_str()))
            .collect();
        assert!(
            !keys.contains(&"POLLED-2"),
            "orphan workflow must be invisible without migration; got keys: {keys:?}"
        );

        let _ = std::fs::remove_dir_all(&data_dir);
    }

    // Clean up env so we don't poison sibling test files in the same binary.
    unsafe {
        std::env::remove_var("TAKUTO_DATA_DIR");
    }
}
