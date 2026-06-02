// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

// Copyright (C) 2026 Alexandre Obellianne
//
// Integration tests for auth-pin + worker secrets bundle.
//
// Bundle unit tests live in `crates/maestro-core/src/auth/bundle.rs::tests`;
// these tests cover the snapshot back-compat invariants, the cross-snapshot
// pin survival, and the integration with `PersistedWorkflowRecord`.

use maestro_core::auth::{MasterKey, seal};
use maestro_core::config::AiAgentProvider;
use maestro_core::workflow::snapshot::{
    AuthPin, PersistedWorkflowRecord, SNAPSHOT_VERSION, WorkflowSnapshotFile,
};

// ---------------------------------------------------------------------------
// T-PIN-002 (P0) — back-compat
// ---------------------------------------------------------------------------

#[test]
fn old_snapshot_without_auth_pin_field_deserializes_as_none() {
    // Hand-written snapshot JSON exactly as pre-Phase-2b.3 builds would have
    // produced. The `auth_pin` field is absent entirely — the new field is
    // `#[serde(default)]` so this must round-trip without errors.
    let raw = r#"{
        "version": 1,
        "workflows": [{
            "id": "wf-1",
            "ticket_key": "OLD-1",
            "ticket_summary": "Legacy workflow",
            "ticket_description": "",
            "ticket_type": "Task",
            "state": "Done",
            "started_at": "2025-01-01T00:00:00Z",
            "updated_at": "2025-01-01T00:00:00Z",
            "steps_log": [],
            "branch_name": "feat/old",
            "worktree_path": null,
            "pr_url": null,
            "terminal_lines": [],
            "current_step_label": null,
            "workspace_name": "legacy-ws"
        }]
    }"#;

    let parsed: WorkflowSnapshotFile =
        serde_json::from_str(raw).expect("pre-Phase-2b.3 snapshot must deserialize");
    assert_eq!(parsed.version, 1);
    assert_eq!(parsed.workflows.len(), 1);
    let w = &parsed.workflows[0];
    assert_eq!(w.ticket_key, "OLD-1");
    assert!(
        w.auth_pin.is_none(),
        "auth_pin must default to None for old snapshots; got {:?}",
        w.auth_pin
    );
    // user_id is also absent in the legacy shape → defaults to None.
    assert!(w.user_id.is_none());
}

// ---------------------------------------------------------------------------
// T-PIN-001 — auth_pin field round-trips through Serde
// ---------------------------------------------------------------------------

#[test]
fn auth_pin_round_trips_through_snapshot_serde() {
    let pin = AuthPin {
        provider: "claude".to_string(),
        provider_credential_row_id: Some(42),
        github_mode: "user_pat".to_string(),
        github_credential_row_id: Some(0),
        started_at: "2026-05-18T12:00:00Z".to_string(),
    };
    let rec = sample_persisted_record_with_pin(Some(pin.clone()));
    let file = WorkflowSnapshotFile {
        version: SNAPSHOT_VERSION,
        workflows: vec![rec],
    };
    let json = serde_json::to_string(&file).unwrap();
    assert!(
        json.contains("\"auth_pin\""),
        "snapshot JSON must include auth_pin field: {json}"
    );

    let parsed: WorkflowSnapshotFile = serde_json::from_str(&json).unwrap();
    let parsed_pin = parsed.workflows[0].auth_pin.as_ref().unwrap();
    assert_eq!(*parsed_pin, pin);
}

// ---------------------------------------------------------------------------
// T-PIN-003 (P0) — pin survives provider switch
// ---------------------------------------------------------------------------

#[test]
fn auth_pin_is_independent_of_active_provider_at_read_time() {
    // The pin records the provider AT START. A later admin change of the
    // active provider doesn't mutate the snapshot's auth_pin.provider —
    // that's the whole point of the pin (04_architecture.md §7.2).
    //
    // We simulate by writing a snapshot with auth_pin.provider="claude",
    // then reading it back through the deserializer — the pin's provider
    // is unchanged regardless of what the live config now says.
    let pin = AuthPin {
        provider: "claude".to_string(),
        provider_credential_row_id: Some(1),
        github_mode: "app".to_string(),
        github_credential_row_id: None,
        started_at: "2026-05-18T12:00:00Z".to_string(),
    };
    let rec = sample_persisted_record_with_pin(Some(pin));
    let file = WorkflowSnapshotFile {
        version: SNAPSHOT_VERSION,
        workflows: vec![rec],
    };
    let json = serde_json::to_string(&file).unwrap();
    let parsed: WorkflowSnapshotFile = serde_json::from_str(&json).unwrap();

    // Even if a sibling AuthPin would now use "cursor", this pin still says
    // "claude" — that's the contract.
    assert_eq!(
        parsed.workflows[0].auth_pin.as_ref().unwrap().provider,
        "claude"
    );

    // Sanity: a fresh AuthPin built from a different provider doesn't
    // interfere with the snapshot's contents.
    let other = AuthPin {
        provider: "cursor".to_string(),
        provider_credential_row_id: None,
        github_mode: "app".to_string(),
        github_credential_row_id: None,
        started_at: "2026-05-18T13:00:00Z".to_string(),
    };
    assert_ne!(
        parsed.workflows[0].auth_pin.as_ref().unwrap().provider,
        other.provider
    );
}

// ---------------------------------------------------------------------------
// pin_for_workflow + bundle::build smoke: live DB end-to-end
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pin_for_workflow_captures_active_provider_from_config() {
    let state = maestro_web::test_helpers::test_state_with_db();
    let db = state.auth().db.as_ref().expect("db").clone();
    let mk = db.master_key().expect("master key").key.clone();

    // Seed user + claude credential.
    seed_user(&db, "u-pin").await;
    let sealed = seal(&mk, b"sk-ant-test").unwrap();
    {
        let adapter = db.adapter();
        let mut tx = adapter.begin().await.unwrap();
        maestro_core::db::provider_credentials::upsert(
            &mut tx,
            "u-pin",
            "claude",
            maestro_core::db::provider_credentials::ProviderCredentialKind::ApiKey,
            &sealed,
            "{}",
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }

    let cfg = {
        let mut c = state.config().config.read().await.clone();
        c.agent.provider = AiAgentProvider::Claude;
        c
    };
    let pin = maestro_core::auth::bundle::pin_for_workflow(&cfg, &db, "u-pin")
        .await
        .expect("pin");
    assert_eq!(pin.provider, "claude");
    assert!(pin.provider_credential_row_id.is_some());
    assert_eq!(pin.github_mode, "app"); // no PAT seeded
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn seed_user(db: &maestro_core::db::Database, user_id: &str) {
    use maestro_core::db::DbValue;
    db.adapter()
        .execute(
            "INSERT INTO users (id, username, role) VALUES (?, ?, 'user')",
            vec![
                DbValue::Text(user_id.to_string()),
                DbValue::Text(user_id.to_string()),
            ],
        )
        .await
        .unwrap();
}

fn sample_persisted_record_with_pin(pin: Option<AuthPin>) -> PersistedWorkflowRecord {
    use chrono::Utc;
    use maestro_core::config::TicketingSystem;
    use maestro_core::workflow::state::WorkflowState;
    use std::collections::HashMap;
    PersistedWorkflowRecord {
        id: uuid::Uuid::new_v4().to_string(),
        ticket_key: "NEW-1".into(),
        ticket_summary: "sample".into(),
        ticket_description: String::new(),
        ticket_type: "Task".into(),
        state: WorkflowState::Done,
        started_at: Utc::now(),
        updated_at: Utc::now(),
        steps_log: vec![],
        branch_name: "feat/new".into(),
        worktree_path: None,
        pr_url: None,
        pr_merged: false,
        terminal_lines: vec![],
        current_step_label: None,
        started_manually: false,
        jira_available: false,
        last_session_id: None,
        description_session_id: None,
        ticketing_system: TicketingSystem::None,
        ticket_url: None,
        driver_started: false,
        workflow_def_runs: HashMap::new(),
        worktree_bootstrapped: false,
        workspace_name: "ws-new".into(),
        repository_id: None,
        user_id: Some("u-pin".into()),
        auth_pin: pin,
    }
}

// Use a Mutex marker so the dead-code lint stays quiet when this file grows.
#[allow(dead_code)]
fn _imports(_: std::marker::PhantomData<MasterKey>) {}
