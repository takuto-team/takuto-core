// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

// Copyright (C) 2026 Alexandre Obellianne
//
// Task #42 — `WorkerSecretsBundle` lifetime guards.
//
// The user's editor terminal showed `/run/maestro-secrets/` was empty
// (`total 4`, only `.` and `..`) because the bundle's `TempDir` was
// dropping after the `start_editor` route returned. The fix stashes the
// bundle's `Arc` into `state.editor().editor_bundles` (and `state.run_command().run_command_bundles`
// for run-command containers) so the host-side directory survives until
// the matching close/stop handler removes the entry.
//
// These integration tests exercise the lifetime contract at the AppState
// layer, without needing DinD: we manually construct a test bundle Arc,
// stash a clone in the map (as `start_editor` does), drop the original,
// assert the TempDir is still on disk, then remove the entry and assert
// the TempDir is gone. This proves the bookkeeping is correct — the
// route-handler code paths in `workflows.rs` exercise the same map
// inserts/removes.

use std::sync::Arc;

use maestro_web::test_helpers::test_state_with_db;

// We can't reach `WorkerSecretsBundle::for_tests` from an integration
// test (it's `#[cfg(test)] pub(crate)` on the core crate), so the
// lifecycle tests below build real bundles via the public
// `build_for_endpoint` path with `allow_shared_default = true` so they
// succeed without a seeded user credential. That gives us a real
// `Arc<WorkerSecretsBundle>` whose `TempDir` we can probe on disk.

/// Sanity: every freshly constructed AppState must start with both
/// bundle maps empty so per-test isolation holds and the route handlers
/// — which insert and remove entries — never see leakage from prior
/// sessions.
#[tokio::test]
async fn t_freshly_constructed_appstate_has_empty_bundle_maps() {
    // Sanity: every freshly constructed AppState must start with both
    // bundle maps empty so per-test isolation holds. The route handlers
    // are responsible for inserting/removing entries; if a previous
    // session somehow leaked, those entries would carry a stale
    // `TempDir` whose host path no longer exists.
    let state = test_state_with_db();
    assert!(
        state.editor().editor_bundles.read().await.is_empty(),
        "freshly constructed AppState must have an empty editor_bundles map"
    );
    assert!(
        state.run_command().run_command_bundles.read().await.is_empty(),
        "freshly constructed AppState must have an empty run_command_bundles map"
    );
}

/// End-to-end TempDir lifecycle: construct a real bundle, stash its Arc
/// in `state.editor().editor_bundles`, drop the original clone (simulating the
/// route handler's stack scope going out of scope), and assert the
/// host-side `TempDir` is still on disk. Then remove the map entry and
/// assert the TempDir is gone.
///
/// This uses `WorkerSecretsBundle::build_for_endpoint` to construct a
/// real bundle. That path needs a DB + master key + resolver; the test
/// helpers set those up. The user has no provider credential, so the
/// bundle will be `Err(provider_credential_missing)` unless we enable
/// `allow_shared_default = true` — which we do here so the build
/// succeeds with an empty secret file.
#[tokio::test]
async fn t_editor_bundle_temp_dir_outlives_route_handler_scope() {
    use maestro_core::auth::WorkerSecretsBundle;

    let state = test_state_with_db();
    // Flip the active provider's `allow_shared_default` so `build_for_endpoint`
    // doesn't insist on a user credential. We're testing lifecycle, not
    // credential resolution.
    {
        let mut cfg = state.config().config.write().await;
        cfg.agent.providers.claude.allow_shared_default = true;
    }
    // Seed a user so `validate_db_session` / `find_active_with_kind` don't
    // panic on a missing user row.
    let db = state.auth().db.clone().expect("test DB");
    let user_id = "u-task-42";
    {
        use maestro_core::db::DbValue;
        let _ = db
            .adapter()
            .execute(
                "INSERT INTO users (id, username, role) VALUES (?, ?, 'user')",
                vec![
                    DbValue::Text(user_id.to_string()),
                    DbValue::Text(user_id.to_string()),
                ],
            )
            .await;
    }

    // Mirror what `build_editor_or_run_command_bundle` does for an
    // editor-open path with no pin (no auth_pin in test).
    let cfg_snapshot = state.config().config.read().await.clone();
    let resolver = state
        .auth()
        .git_auth_resolver
        .as_ref()
        .expect("test helper installs a resolver")
        .clone();
    let bundle = maestro_core::auth::bundle::build_for_endpoint(
        &cfg_snapshot,
        state.auth().db.as_ref().unwrap(),
        &resolver,
        user_id,
    )
    .await
    .expect("build_for_endpoint must succeed with allow_shared_default=true");
    let bundle_arc: Arc<WorkerSecretsBundle> = Arc::new(bundle);
    let host_dir = bundle_arc.host_dir().to_path_buf();
    assert!(
        host_dir.exists(),
        "TempDir must exist immediately after construction"
    );

    // Stash a clone the way `start_editor` does.
    let ticket = "task-42-ticket".to_string();
    {
        let mut map = state.editor().editor_bundles.write().await;
        map.insert(ticket.clone(), bundle_arc.clone());
    }

    // Drop the route-handler's local Arc.
    drop(bundle_arc);

    // The TempDir MUST still be on disk — the map's clone keeps it alive.
    assert!(
        host_dir.exists(),
        "TempDir must survive after the route-handler clone drops; the AppState map is now the sole owner"
    );

    // Simulate close_editor: remove the map entry.
    let removed = state.editor().editor_bundles.write().await.remove(&ticket);
    assert!(removed.is_some(), "map entry must be present before close");
    drop(removed);

    // RAII fires now.
    assert!(
        !host_dir.exists(),
        "TempDir must be rm -rf'd once the map entry is removed (last Arc strong reference)"
    );
}

/// Same lifecycle test, but for the run-command bundle map. The key is a
/// tuple `(ticket_key, cmd_index)` since a workflow can have multiple
/// concurrent run-commands. Removing one entry must NOT drop the other.
#[tokio::test]
async fn t_run_command_bundle_map_is_keyed_by_ticket_and_index() {
    let state = test_state_with_db();
    {
        let mut cfg = state.config().config.write().await;
        cfg.agent.providers.claude.allow_shared_default = true;
    }
    let db = state.auth().db.clone().expect("test DB");
    let user_id = "u-task-42b";
    {
        use maestro_core::db::DbValue;
        let _ = db
            .adapter()
            .execute(
                "INSERT INTO users (id, username, role) VALUES (?, ?, 'user')",
                vec![
                    DbValue::Text(user_id.to_string()),
                    DbValue::Text(user_id.to_string()),
                ],
            )
            .await;
    }

    // Build two distinct bundles (one for cmd_index 0, one for cmd_index 1).
    let cfg_snapshot = state.config().config.read().await.clone();
    let resolver = state.auth().git_auth_resolver.as_ref().unwrap().clone();
    let mk_bundle = || async {
        let b = maestro_core::auth::bundle::build_for_endpoint(
            &cfg_snapshot,
            state.auth().db.as_ref().unwrap(),
            &resolver,
            user_id,
        )
        .await
        .unwrap();
        Arc::new(b)
    };
    let a = mk_bundle().await;
    let b = mk_bundle().await;
    let a_dir = a.host_dir().to_path_buf();
    let b_dir = b.host_dir().to_path_buf();
    assert_ne!(a_dir, b_dir, "test setup: each bundle must own a unique TempDir");

    let ticket = "task-42b-ticket".to_string();
    {
        let mut map = state.run_command().run_command_bundles.write().await;
        map.insert((ticket.clone(), 0), a.clone());
        map.insert((ticket.clone(), 1), b.clone());
    }
    drop(a);
    drop(b);

    // Both dirs still alive — map holds the only Arcs.
    assert!(a_dir.exists());
    assert!(b_dir.exists());

    // Remove just cmd_index 0 — the other must survive.
    state
        .run_command()
        .run_command_bundles
        .write()
        .await
        .remove(&(ticket.clone(), 0));
    assert!(!a_dir.exists(), "removed entry's TempDir must be gone");
    assert!(b_dir.exists(), "untouched entry's TempDir must survive");

    // Workflow teardown wipes everything for this ticket.
    state
        .run_command()
        .run_command_bundles
        .write()
        .await
        .retain(|(tk, _), _| tk != &ticket);
    assert!(!b_dir.exists(), "workflow teardown must drop remaining entries");
}
