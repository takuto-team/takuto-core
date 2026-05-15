// Copyright (C) 2026 Alexandre Obellianne
//
// Plan-10 integration test for the startup reconciliation pass.
//
// Two passes are exercised against a real SQLite DB + a real on-disk fake
// workspaces directory + a real snapshot file:
//
// 1. `repo_reconcile::reconcile_repositories` discovers every `<dir>/.git`
//    under the workspaces path and registers one row in `repositories` per
//    discovery. Running it twice is a no-op (G6 — atomic on conflict).
//
// 2. `repo_reconcile::backfill_user_repositories_from_snapshots` reads the
//    persisted workflow snapshot at
//    `{data_dir}/workspaces/<name>/workflow_snapshot.json`, finds each record
//    with `user_id = Some(uid)` and a `workspace_name` matching a registered
//    repo, and inserts the `(user_id, repository_id)` association. Running
//    it twice is a no-op.

use std::path::PathBuf;

use chrono::Utc;
use maestro_core::db::Database;
use maestro_core::db::models::UserRole;
use maestro_core::db::repositories;
use maestro_core::db::users::create_user;
use maestro_core::repo_reconcile::{
    backfill_user_repositories_from_snapshots, reconcile_repositories,
};
use maestro_core::workflow::snapshot::{
    PersistedWorkflowRecord, SNAPSHOT_FILENAME, SNAPSHOT_VERSION, WorkflowSnapshotFile,
};
use maestro_core::workflow::state::WorkflowState;

fn fresh_data_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "maestro-plan10-recon-{}-{}",
        label,
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).expect("create temp data dir");
    dir
}

/// Create a fake `<workspaces_dir>/<name>/.git/` directory shaped just enough
/// to look like a real clone to the reconciliation scan. A minimal
/// `.git/config` with an `origin` remote lets us also verify URL discovery.
fn make_fake_clone(workspaces_dir: &std::path::Path, name: &str, origin_url: &str) {
    let repo = workspaces_dir.join(name);
    let git_dir = repo.join(".git");
    std::fs::create_dir_all(&git_dir).expect("create .git dir");
    std::fs::write(
        git_dir.join("config"),
        format!(
            r#"[core]
    repositoryformatversion = 0
[remote "origin"]
    url = {origin_url}
    fetch = +refs/heads/*:refs/remotes/origin/*
"#
        ),
    )
    .expect("write .git/config");
}

/// Pre-seed a workspace snapshot containing one Done workflow whose `user_id`
/// is set and whose `workspace_name` matches a registered repo. Done is used
/// so engine restore (in other contexts) doesn't spawn a driver — for this
/// test only the file's existence and contents matter.
fn seed_snapshot_workflow(
    data_dir: &std::path::Path,
    workspace_name: &str,
    user_id: &str,
    ticket_key: &str,
) {
    let ws_dir = data_dir.join("workspaces").join(workspace_name);
    std::fs::create_dir_all(&ws_dir).expect("create workspace dir");

    let rec = PersistedWorkflowRecord {
        id: uuid::Uuid::new_v4().to_string(),
        ticket_key: ticket_key.to_string(),
        ticket_summary: "Snapshot for plan-10 backfill".into(),
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
        ticketing_system: maestro_core::config::TicketingSystem::None,
        ticket_url: None,
        driver_started: false,
        workflow_def_runs: std::collections::HashMap::new(),
        worktree_bootstrapped: false,
        workspace_name: workspace_name.to_string(),
        repository_id: None,
        user_id: Some(user_id.to_string()),
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

#[tokio::test]
async fn reconcile_then_backfill_e2e() {
    // Build a temp `data_dir` + a sibling `workspaces` directory.
    let data_dir = fresh_data_dir("e2e");
    let workspaces_dir = data_dir.join("on_disk_workspaces");
    std::fs::create_dir_all(&workspaces_dir).expect("create workspaces dir");

    // Two fake clones.
    make_fake_clone(
        &workspaces_dir,
        "alpha",
        "git@github.com:owner-a/alpha.git",
    );
    make_fake_clone(
        &workspaces_dir,
        "beta",
        "https://github.com/owner-b/beta.git",
    );
    // And one entry that ISN'T a git clone — must be ignored by the scan.
    std::fs::create_dir_all(workspaces_dir.join("not-a-repo")).unwrap();
    std::fs::write(workspaces_dir.join("not-a-repo/README.md"), "hi").unwrap();

    let db = Database::open(&data_dir).expect("open DB");

    let workspaces_dir_str = workspaces_dir.to_str().expect("utf-8");

    // ── First pass: reconcile_repositories registers both clones. ──
    {
        let conn_arc = db.conn().clone();
        let conn_guard = conn_arc.lock().await;
        let conn = &*conn_guard;

        let inserted = reconcile_repositories(conn, workspaces_dir_str)
            .expect("reconcile_repositories must succeed");
        assert_eq!(
            inserted, 2,
            "expected 2 fresh inserts (alpha, beta); got {inserted}"
        );

        let all = repositories::list_all(conn).expect("list_all");
        assert_eq!(all.len(), 2, "expected 2 repository rows; got {}", all.len());
        let names: Vec<&str> = all.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"alpha"));
        assert!(names.contains(&"beta"));

        // URL discovery worked for both: SSH form was normalised to HTTPS.
        let alpha = all
            .iter()
            .find(|r| r.name == "alpha")
            .expect("alpha row exists");
        assert_eq!(
            alpha.repo_url.as_deref(),
            Some("https://github.com/owner-a/alpha"),
            "SSH origin URL must be normalised"
        );
        let beta = all
            .iter()
            .find(|r| r.name == "beta")
            .expect("beta row exists");
        assert_eq!(
            beta.repo_url.as_deref(),
            Some("https://github.com/owner-b/beta"),
            "HTTPS origin URL stripped of .git suffix"
        );

        // No user_repositories rows yet (orphan registrations).
        let ur_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM user_repositories", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ur_count, 0);
    }

    // ── Second pass: idempotency. ──
    {
        let conn_arc = db.conn().clone();
        let conn_guard = conn_arc.lock().await;
        let conn = &*conn_guard;

        let inserted_again = reconcile_repositories(conn, workspaces_dir_str)
            .expect("second reconcile_repositories must succeed");
        assert_eq!(
            inserted_again, 0,
            "second pass must insert 0 (idempotent); got {inserted_again}"
        );

        let all = repositories::list_all(conn).expect("list_all");
        assert_eq!(all.len(), 2, "still 2 rows after second pass");
    }

    // ── Snapshot backfill: a workflow file points at the "alpha" workspace
    // and has a user_id. backfill must create a `(uid, alpha.id)` row.
    let (alice_id, alpha_id) = {
        let conn_arc = db.conn().clone();
        let conn_guard = conn_arc.lock().await;
        let conn = &*conn_guard;
        let alice_id = create_user(conn, "alice", UserRole::User).expect("create alice").id;
        let alpha_id = repositories::get_by_name(conn, "alpha")
            .expect("get_by_name")
            .expect("alpha row exists")
            .id;
        (alice_id, alpha_id)
    };

    // Write a snapshot referencing the "alpha" workspace + alice.
    seed_snapshot_workflow(&data_dir, "alpha", &alice_id, "ALPHA-1");

    // backfill picks up the snapshot.
    {
        let conn_arc = db.conn().clone();
        let conn_guard = conn_arc.lock().await;
        let conn = &*conn_guard;

        let n = backfill_user_repositories_from_snapshots(conn, &data_dir)
            .expect("backfill must succeed");
        assert_eq!(n, 1, "expected 1 backfilled association; got {n}");

        let mine = repositories::list_for_user(conn, &alice_id).expect("list_for_user");
        assert_eq!(mine.len(), 1, "alice now has 1 added repo");
        assert_eq!(mine[0].id, alpha_id);
    }

    // ── Backfill is idempotent. ──
    {
        let conn_arc = db.conn().clone();
        let conn_guard = conn_arc.lock().await;
        let conn = &*conn_guard;

        let n_again = backfill_user_repositories_from_snapshots(conn, &data_dir)
            .expect("second backfill must succeed");
        assert_eq!(
            n_again, 0,
            "second pass must insert 0 (idempotent); got {n_again}"
        );

        let mine = repositories::list_for_user(conn, &alice_id).expect("list_for_user");
        assert_eq!(mine.len(), 1, "still 1 association after second backfill");
    }

    // Cleanup.
    let _ = std::fs::remove_dir_all(&data_dir);
}

/// Snapshots whose `workspace_name` doesn't match any registered repository
/// are skipped silently — the backfill returns 0 and no row is added (AC-17).
#[tokio::test]
async fn backfill_skips_unmatched_workspaces() {
    let data_dir = fresh_data_dir("unmatched");

    let db = Database::open(&data_dir).expect("open DB");

    // Register "alpha" only.
    let alice_id = {
        let conn_arc = db.conn().clone();
        let conn_guard = conn_arc.lock().await;
        let conn = &*conn_guard;
        let alice_id = create_user(conn, "alice", UserRole::User).expect("alice").id;
        repositories::upsert(
            conn,
            "alpha",
            None,
            "/workspaces/alpha",
            "main",
            None,
        )
        .expect("upsert alpha");
        alice_id
    };

    // Seed a snapshot for an UNREGISTERED workspace ("gamma").
    seed_snapshot_workflow(&data_dir, "gamma", &alice_id, "GAMMA-1");

    // Run backfill. The workflow's workspace_name doesn't match any
    // registered repo → backfill returns 0 and no association row is added.
    {
        let conn_arc = db.conn().clone();
        let conn_guard = conn_arc.lock().await;
        let conn = &*conn_guard;
        let n = backfill_user_repositories_from_snapshots(conn, &data_dir)
            .expect("backfill must succeed");
        assert_eq!(
            n, 0,
            "snapshot pointing at unregistered workspace must not insert; got {n}"
        );
        let mine = repositories::list_for_user(conn, &alice_id).expect("list_for_user");
        assert!(
            mine.is_empty(),
            "alice has no associations (unmatched snapshot)"
        );
    }

    let _ = std::fs::remove_dir_all(&data_dir);
}

/// Snapshots without `user_id` are skipped — there's no owner to associate.
#[tokio::test]
async fn backfill_skips_orphan_workflows() {
    let data_dir = fresh_data_dir("orphan");
    let db = Database::open(&data_dir).expect("open DB");

    {
        let conn_arc = db.conn().clone();
        let conn_guard = conn_arc.lock().await;
        let conn = &*conn_guard;
        repositories::upsert(conn, "alpha", None, "/workspaces/alpha", "main", None)
            .expect("upsert");
    }

    // Snapshot with no user_id (legacy orphan workflow).
    let ws_dir = data_dir.join("workspaces").join("alpha");
    std::fs::create_dir_all(&ws_dir).expect("create");
    let rec = PersistedWorkflowRecord {
        id: uuid::Uuid::new_v4().to_string(),
        ticket_key: "ORPHAN-1".into(),
        ticket_summary: "no owner".into(),
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
        ticketing_system: maestro_core::config::TicketingSystem::None,
        ticket_url: None,
        driver_started: false,
        workflow_def_runs: std::collections::HashMap::new(),
        worktree_bootstrapped: false,
        workspace_name: "alpha".into(),
        repository_id: None,
        user_id: None,
    };
    let file = WorkflowSnapshotFile {
        version: SNAPSHOT_VERSION,
        workflows: vec![rec],
    };
    std::fs::write(
        ws_dir.join(SNAPSHOT_FILENAME),
        serde_json::to_vec_pretty(&file).unwrap(),
    )
    .unwrap();

    {
        let conn_arc = db.conn().clone();
        let conn_guard = conn_arc.lock().await;
        let conn = &*conn_guard;
        let n = backfill_user_repositories_from_snapshots(conn, &data_dir)
            .expect("backfill must succeed");
        assert_eq!(n, 0, "orphan snapshot must be skipped; got {n}");

        let ur_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM user_repositories", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ur_count, 0);
    }

    let _ = std::fs::remove_dir_all(&data_dir);
}
