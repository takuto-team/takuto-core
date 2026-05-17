// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

// Copyright (C) 2026 Alexandre Obellianne
//
// Integration tests for plan-09 Step 3 (engine resolution of
// `worktree_init_commands` from the workflow owner's per-user-per-workspace
// DB row) and Step 4 (run-command surfacing on `WorkflowSummary` reads from
// the same DB row).
//
// Acceptance criteria covered:
//
//   * AC-3 — a user with no `user_worktree_commands` row runs zero init
//            commands during bootstrap and has zero run-command buttons.
//   * AC-4 — saving a row makes the next bootstrap use its init_commands,
//            and the workflow card shows the run-command buttons.
//   * AC-8 — `WorkflowSummary.run_commands` on the list endpoint reflects
//            the workflow owner's DB row (not any global config).
//
// We test the resolution helper directly (`resolve_worktree_init_commands`)
// instead of driving the full bootstrap pipeline because the latter requires
// a live Docker / DinD environment that is not available in `cargo test`.
// The helper IS the resolution logic exercised by the driver (see
// `bootstrap_new_workflow` in
// `crates/maestro-core/src/workflow/engine/driver.rs`).

use maestro_core::db::models::UserRole;
use maestro_core::db::{Database, user_worktree_commands, users};
use maestro_core::workflow::engine::resolve_worktree_init_commands;
use maestro_web::test_helpers::temp_db;

/// Create a user with the given username and return the generated id.
async fn make_user(db: &Database, username: &str) -> String {
    let conn = db.conn().lock().await;
    users::create_user(&conn, username, UserRole::User)
        .expect("create_user")
        .id
}

/// AC-3: when there's no DB row for the workflow's `(user_id, workspace)`
/// pair, the resolver returns an empty vec.
#[tokio::test]
async fn resolver_returns_empty_when_owner_has_no_row() {
    let db = temp_db();
    let alice = make_user(&db, "alice").await;

    let resolved = resolve_worktree_init_commands(Some(&alice), "frontend", Some(&db)).await;

    assert!(
        resolved.is_empty(),
        "with no row the resolver must run zero init commands; got {resolved:?}"
    );
}

/// AC-4 / Step-3 happy path: when the owner has a row, the resolver returns
/// its `init_commands` verbatim.
#[tokio::test]
async fn resolver_returns_init_commands_from_owner_row() {
    let db = temp_db();
    let alice = make_user(&db, "alice").await;

    {
        let conn = db.conn().lock().await;
        user_worktree_commands::upsert(
            &conn,
            &alice,
            "frontend",
            &[
                "echo from-row-1".to_string(),
                "echo from-row-2".to_string(),
            ],
            &[],
        )
        .expect("upsert owner row");
    }

    let resolved = resolve_worktree_init_commands(Some(&alice), "frontend", Some(&db)).await;

    assert_eq!(
        resolved,
        vec![
            "echo from-row-1".to_string(),
            "echo from-row-2".to_string(),
        ],
        "the resolver must return the owner's init_commands verbatim"
    );
}

/// AC-3 cont.: an orphan workflow (`user_id == None`) runs zero init commands,
/// even if there's a row belonging to a different user for the same workspace.
#[tokio::test]
async fn resolver_returns_empty_when_workflow_has_no_user() {
    let db = temp_db();
    let alice = make_user(&db, "alice").await;

    {
        let conn = db.conn().lock().await;
        user_worktree_commands::upsert(
            &conn,
            &alice,
            "frontend",
            &["echo not-for-orphan".to_string()],
            &[],
        )
        .expect("upsert alice's row");
    }

    let resolved = resolve_worktree_init_commands(None, "frontend", Some(&db)).await;

    assert!(
        resolved.is_empty(),
        "an orphan workflow must never pick up another user's commands; got {resolved:?}"
    );
}

/// Sanity: with no DB at all (`None`), the resolver returns an empty vec
/// (matches the production behavior on deployments where the DB failed to
/// open). There is no longer a global-default fallback (plan-09).
#[tokio::test]
async fn resolver_returns_empty_when_db_missing() {
    let resolved = resolve_worktree_init_commands(Some("user-alice"), "frontend", None).await;

    assert!(
        resolved.is_empty(),
        "without a DB the resolver must run zero init commands; got {resolved:?}"
    );
}

/// AC-8: user A's row never leaks into user B's resolution — even on the
/// same workspace.
#[tokio::test]
async fn resolver_isolates_owners() {
    let db = temp_db();
    let alice = make_user(&db, "alice").await;
    let bob = make_user(&db, "bob").await;

    {
        let conn = db.conn().lock().await;
        user_worktree_commands::upsert(
            &conn,
            &alice,
            "frontend",
            &["echo alice".to_string()],
            &[],
        )
        .expect("upsert alice");
        user_worktree_commands::upsert(
            &conn,
            &bob,
            "frontend",
            &["echo bob".to_string()],
            &[],
        )
        .expect("upsert bob");
    }

    let for_alice =
        resolve_worktree_init_commands(Some(&alice), "frontend", Some(&db)).await;
    let for_bob = resolve_worktree_init_commands(Some(&bob), "frontend", Some(&db)).await;

    assert_eq!(for_alice, vec!["echo alice".to_string()]);
    assert_eq!(for_bob, vec!["echo bob".to_string()]);
}

/// AC-8 (run-command surfacing): the DB row stores both init and run
/// commands; the run-command side is what `WorkflowSummary.run_commands`
/// reflects. Owner has a row → run commands are exactly those in
/// `row.run_commands`; owner has no row → empty.
#[tokio::test]
async fn run_commands_surface_from_owner_row() {
    let db = temp_db();
    let alice = make_user(&db, "alice").await;
    let bob = make_user(&db, "bob").await;
    let rc = user_worktree_commands::RunCommand {
        name: "Dev Server".to_string(),
        command: "npm run dev".to_string(),
    };

    {
        let conn = db.conn().lock().await;
        user_worktree_commands::upsert(
            &conn,
            &alice,
            "frontend",
            &["echo init".to_string()],
            &[rc.clone()],
        )
        .expect("upsert with run command");
    }

    // What the workflows handler does: read the row, take `.run_commands`.
    let row = {
        let conn = db.conn().lock().await;
        user_worktree_commands::get(&conn, &alice, "frontend")
            .expect("query owner row")
            .expect("row must exist")
    };
    assert_eq!(row.run_commands, vec![rc.clone()]);

    // No row → empty run-commands list (the buttons disappear).
    let missing = {
        let conn = db.conn().lock().await;
        user_worktree_commands::get(&conn, &bob, "frontend").expect("query bob")
    };
    assert!(missing.is_none(), "bob has no row → no run commands");
}

/// AC-8 (run-command surfacing, batched): the list-endpoint path uses
/// `get_run_commands_for_pairs` to batch-load run commands for every
/// workflow on the dashboard in a single query. This is the same data
/// that the per-workflow lookup returns — just batched.
#[tokio::test]
async fn run_commands_batched_lookup_returns_per_owner_data() {
    let db = temp_db();
    let alice = make_user(&db, "alice").await;
    let bob = make_user(&db, "bob").await;
    let carol = make_user(&db, "carol").await;

    let alice_rc = user_worktree_commands::RunCommand {
        name: "Storybook".into(),
        command: "npx storybook dev -p 6006".into(),
    };
    let bob_rc = user_worktree_commands::RunCommand {
        name: "Dev".into(),
        command: "npm run dev".into(),
    };

    {
        let conn = db.conn().lock().await;
        user_worktree_commands::upsert(
            &conn,
            &alice,
            "frontend",
            &[],
            &[alice_rc.clone()],
        )
        .unwrap();
        user_worktree_commands::upsert(
            &conn,
            &bob,
            "frontend",
            &[],
            &[bob_rc.clone()],
        )
        .unwrap();
        // carol has no row, on purpose.
    }

    let pairs: Vec<(&str, &str)> = vec![
        (alice.as_str(), "frontend"),
        (bob.as_str(), "frontend"),
        (carol.as_str(), "frontend"), // miss — no row
    ];

    let by_pair = {
        let conn = db.conn().lock().await;
        user_worktree_commands::get_run_commands_for_pairs(&conn, &pairs)
            .expect("batched lookup")
    };

    assert_eq!(
        by_pair.get(&(alice.clone(), "frontend".to_string())),
        Some(&vec![alice_rc])
    );
    assert_eq!(
        by_pair.get(&(bob.clone(), "frontend".to_string())),
        Some(&vec![bob_rc])
    );
    assert!(
        !by_pair.contains_key(&(carol.clone(), "frontend".to_string())),
        "carol has no row → not in the map (caller treats absence as empty)"
    );
}
