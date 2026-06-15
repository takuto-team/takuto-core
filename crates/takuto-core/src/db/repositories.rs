// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Per-user repository registry.
//!
//! Two tables, one module:
//!
//! - `repositories` — one row per on-disk clone under `WORKSPACES_DIR`. Identity
//!   is the UUID `id`; `local_path` is the on-disk uniqueness key (UNIQUE).
//!   `name` is **not unique** — two forks (`owner-a/foo` vs `owner-b/foo`) can
//!   both register with `name=foo` on different `local_path`s.
//! - `user_repositories` — many-to-many between users and repositories.
//!   Composite PK `(user_id, repository_id)`, both FKs cascading on user or
//!   repository delete.
//!
//! The dashboard's "my repositories" tab and the workflow-list filter both
//! drive off this table.
//!
//! ### Backend compatibility
//!
//! The `INSERT ... ON CONFLICT(col) DO NOTHING` form used by `upsert` and
//! `add_for_user` is SQLite + Postgres-compatible; MySQL would need
//! `ON DUPLICATE KEY UPDATE` (documented inline, deferred until MySQL CI
//! lands).

use uuid::Uuid;

use crate::db::{DbAdapter, DbValue};
use crate::error::Result;

/// One row in `repositories`. Identity is `id` (UUID); `local_path` is the
/// on-disk uniqueness key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryRow {
    pub id: String,
    pub name: String,
    pub repo_url: Option<String>,
    pub local_path: String,
    pub default_branch: String,
    pub created_at: i64,
    pub created_by: Option<String>,
}

/// One row in `user_repositories`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserRepositoryRow {
    pub user_id: String,
    pub repository_id: String,
    pub added_at: i64,
}

// ── repositories CRUD ───────────────────────────────────────────────────────

/// Fetch a repository by id.
pub async fn get(adapter: &DbAdapter, id: &str) -> Result<Option<RepositoryRow>> {
    let row = adapter
        .query_optional(
            "SELECT id, name, repo_url, local_path, default_branch, created_at, created_by \
             FROM repositories WHERE id = ?",
            vec![DbValue::Text(id.to_string())],
        )
        .await?;
    row.map(|r| decode_repository(&r)).transpose()
}

/// Fetch a repository by `name`. Returns the first match — `name` is NOT
/// unique by design, so callers expecting ambiguity should use `list_all`.
/// Used by the workflow filter to map a snapshot's `workspace_name` →
/// `repository_id`.
pub async fn get_by_name(adapter: &DbAdapter, name: &str) -> Result<Option<RepositoryRow>> {
    let row = adapter
        .query_optional(
            "SELECT id, name, repo_url, local_path, default_branch, created_at, created_by \
             FROM repositories WHERE name = ? ORDER BY created_at ASC LIMIT 1",
            vec![DbValue::Text(name.to_string())],
        )
        .await?;
    row.map(|r| decode_repository(&r)).transpose()
}

/// Fetch a repository by `local_path` (which is UNIQUE).
pub async fn get_by_path(adapter: &DbAdapter, local_path: &str) -> Result<Option<RepositoryRow>> {
    let row = adapter
        .query_optional(
            "SELECT id, name, repo_url, local_path, default_branch, created_at, created_by \
             FROM repositories WHERE local_path = ?",
            vec![DbValue::Text(local_path.to_string())],
        )
        .await?;
    row.map(|r| decode_repository(&r)).transpose()
}

/// List every registered repository, ordered by `created_at ASC, name ASC` for
/// determinism.
pub async fn list_all(adapter: &DbAdapter) -> Result<Vec<RepositoryRow>> {
    let rows = adapter
        .query_all(
            "SELECT id, name, repo_url, local_path, default_branch, created_at, created_by \
             FROM repositories ORDER BY created_at ASC, name ASC",
            vec![],
        )
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        out.push(decode_repository(r)?);
    }
    Ok(out)
}

/// Insert a repository if one doesn't already exist at `local_path` (the
/// on-disk uniqueness key). Returns the existing or newly-created `id`.
///
/// Atomic against concurrent first-boot reconciliation: the `INSERT … ON
/// CONFLICT(local_path) DO NOTHING` step either commits the new row or
/// observes the row a racing peer just inserted. The follow-up `SELECT id`
/// always returns a value because `local_path` is UNIQUE.
///
/// Other columns are *only* applied on insert — if a row already exists at
/// `local_path`, the existing `name` / `repo_url` / `default_branch` /
/// `created_by` are preserved. Use a separate update path if those need to
/// change (none exists today).
///
/// Backend support: SQLite (≥ 3.24) and Postgres use `ON CONFLICT(...) DO
/// NOTHING` verbatim. MySQL would use `INSERT IGNORE` — when MySQL is
/// added, this fn needs a `match adapter.backend()` branch.
pub async fn upsert(
    adapter: &DbAdapter,
    name: &str,
    repo_url: Option<&str>,
    local_path: &str,
    default_branch: &str,
    created_by: Option<&str>,
) -> Result<String> {
    let new_id = Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp();

    let tail = super::upsert::build_ignore_tail(adapter.backend(), &["local_path"]);
    let sql = format!(
        "INSERT INTO repositories \
            (id, name, repo_url, local_path, default_branch, created_at, created_by) \
         VALUES (?, ?, ?, ?, ?, ?, ?) {tail}"
    );
    adapter
        .execute(
            &sql,
            vec![
                DbValue::Text(new_id),
                DbValue::Text(name.to_string()),
                DbValue::TextOpt(repo_url.map(|s| s.to_string())),
                DbValue::Text(local_path.to_string()),
                DbValue::Text(default_branch.to_string()),
                DbValue::I64(now),
                DbValue::TextOpt(created_by.map(|s| s.to_string())),
            ],
        )
        .await?;

    // Read back the row's id — either the one we just inserted or the one
    // a racing peer inserted. `local_path` is UNIQUE so this query is
    // single-row.
    let row = adapter
        .query_one(
            "SELECT id FROM repositories WHERE local_path = ?",
            vec![DbValue::Text(local_path.to_string())],
        )
        .await?;
    Ok(row.get_text(0)?)
}

/// Delete a `repositories` row by id. Returns `true` if a row was deleted,
/// `false` if no such id existed. Cascades to `user_repositories` via FK.
///
/// This is a DB-only delete; the on-disk clone is removed by the calling
/// REST handler when appropriate (always-purge semantics).
pub async fn delete(adapter: &DbAdapter, id: &str) -> Result<bool> {
    let affected = adapter
        .execute(
            "DELETE FROM repositories WHERE id = ?",
            vec![DbValue::Text(id.to_string())],
        )
        .await?;
    Ok(affected > 0)
}

// ── user_repositories CRUD ──────────────────────────────────────────────────

/// List every repository the user has added, ordered by `added_at DESC` (most
/// recently added first — matches the start-manual default repo selection).
///
/// `added_at` is stored in whole seconds, so two calls to `add_for_user`
/// within the same second produce ties. The secondary sort on
/// `ur.repository_id DESC` gives every backend a deterministic order
/// for tied rows — alphanumeric on the UUID, not insertion order, but
/// stable across the cluster of backends we support. (The previous
/// `ur.rowid DESC` was SQLite-only and broke `My Repositories` on
/// Postgres with `column ur.rowid does not exist`.)
pub async fn list_for_user(adapter: &DbAdapter, user_id: &str) -> Result<Vec<RepositoryRow>> {
    let rows = adapter
        .query_all(
            "SELECT r.id, r.name, r.repo_url, r.local_path, r.default_branch, r.created_at, r.created_by \
             FROM repositories r \
             INNER JOIN user_repositories ur ON ur.repository_id = r.id \
             WHERE ur.user_id = ? \
             ORDER BY ur.added_at DESC, ur.repository_id DESC",
            vec![DbValue::Text(user_id.to_string())],
        )
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        out.push(decode_repository(r)?);
    }
    Ok(out)
}

/// List every registered repository the user has NOT yet added. Used by the
/// "Available repositories" picker in the My Repositories tab.
pub async fn list_available_for_user(
    adapter: &DbAdapter,
    user_id: &str,
) -> Result<Vec<RepositoryRow>> {
    let rows = adapter
        .query_all(
            "SELECT r.id, r.name, r.repo_url, r.local_path, r.default_branch, r.created_at, r.created_by \
             FROM repositories r \
             WHERE NOT EXISTS ( \
                 SELECT 1 FROM user_repositories ur \
                 WHERE ur.repository_id = r.id AND ur.user_id = ? \
             ) \
             ORDER BY r.created_at ASC, r.name ASC",
            vec![DbValue::Text(user_id.to_string())],
        )
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        out.push(decode_repository(r)?);
    }
    Ok(out)
}

/// Associate `repository_id` with `user_id`. Returns `true` when a new row was
/// inserted, `false` when the association already existed (idempotent).
///
/// Uses `INSERT ... ON CONFLICT(user_id, repository_id) DO NOTHING`, so racing
/// concurrent adds collapse to a single row. (MySQL caveat per `upsert`.)
pub async fn add_for_user(adapter: &DbAdapter, user_id: &str, repository_id: &str) -> Result<bool> {
    let now = chrono::Utc::now().timestamp();
    let tail = super::upsert::build_ignore_tail(adapter.backend(), &["user_id", "repository_id"]);
    let sql = format!(
        "INSERT INTO user_repositories (user_id, repository_id, added_at) \
         VALUES (?, ?, ?) {tail}"
    );
    let affected = adapter
        .execute(
            &sql,
            vec![
                DbValue::Text(user_id.to_string()),
                DbValue::Text(repository_id.to_string()),
                DbValue::I64(now),
            ],
        )
        .await?;
    Ok(affected > 0)
}

/// Drop the association between `user_id` and `repository_id`. Returns `true`
/// when a row was deleted, `false` when no association existed.
///
/// This is a single-link delete only — it doesn't cascade to the
/// `repositories` row or the on-disk clone. The REST layer decides whether to
/// purge based on "last user remove" semantics.
pub async fn remove_for_user(
    adapter: &DbAdapter,
    user_id: &str,
    repository_id: &str,
) -> Result<bool> {
    let affected = adapter
        .execute(
            "DELETE FROM user_repositories WHERE user_id = ? AND repository_id = ?",
            vec![
                DbValue::Text(user_id.to_string()),
                DbValue::Text(repository_id.to_string()),
            ],
        )
        .await?;
    Ok(affected > 0)
}

/// Does the user have any registered repository with this `name`?
///
/// Defensive back-compat path: workflow snapshots persist `workspace_name`
/// (a string) rather than `repository_id` for legacy rows. The repository
/// filter uses this when the workflow's `repository_id` is absent. Returns
/// `true` if at least one repository with that `name` is in the user's
/// added set.
pub async fn user_has(adapter: &DbAdapter, user_id: &str, repository_name: &str) -> Result<bool> {
    let row = adapter
        .query_one(
            "SELECT COUNT(*) FROM repositories r \
             INNER JOIN user_repositories ur ON ur.repository_id = r.id \
             WHERE ur.user_id = ? AND r.name = ?",
            vec![
                DbValue::Text(user_id.to_string()),
                DbValue::Text(repository_name.to_string()),
            ],
        )
        .await?;
    Ok(row.get_i64(0)? > 0)
}

/// Return every active (non-terminal) workflow on `repository_id`, as
/// `(ticket_key, user_id)` pairs. Used by the DELETE-repo handler to refuse
/// removal when active work is in progress.
///
/// "Active" = not Done, Stopped, or Error. We probe the per-workspace snapshot
/// files via `workspace_name` matching (durable for restored snapshots), then
/// filter in-memory by state. The snapshot row stores the workflow state as a
/// debug string (e.g. `"Done"`, `"Stopped"`, `"Error"`, `"Init"`, ...).
///
/// **Caveat**: at the time of writing, workflow snapshots persist by
/// `workspace_name` (the on-disk dir name) rather than `repository_id`. We
/// resolve `local_path → name → snapshot_dir` so this works for both
/// legacy and current snapshot layouts. If `repository_id` is later added
/// to the snapshot model, this helper should be updated to match on
/// `repository_id` first and fall back to `workspace_name`.
pub async fn repository_has_active_workflow(
    adapter: &DbAdapter,
    repository_id: &str,
) -> Result<Vec<(String, String)>> {
    // 1. Resolve `repository_id` → `name` + `local_path`.
    let row = match get(adapter, repository_id).await? {
        Some(r) => r,
        None => return Ok(Vec::new()),
    };

    // 2. Discover the data_dir to read per-workspace snapshots from. If no
    //    data dir is configured we can't tell (e.g. tests in-memory) → return
    //    empty list.
    let Some(data_dir) = crate::workflow::snapshot::resolve_data_dir() else {
        return Ok(Vec::new());
    };

    // 3. Read every snapshot record from disk, filter to ones whose
    //    workspace_name matches our repository's `name` AND whose state is
    //    active.
    let records =
        crate::workflow::snapshot::read_all_workspace_snapshots(&data_dir).unwrap_or_default();

    let mut blockers = Vec::new();
    for rec in records {
        if rec.workspace_name != row.name {
            continue;
        }
        // Terminal = Done / Stopped / Error per `WorkflowState::is_terminal`.
        // Anything else (including Paused) blocks deletion.
        if !rec.state.is_terminal() {
            let uid = rec.user_id.clone().unwrap_or_default();
            blockers.push((rec.ticket_key.clone(), uid));
        }
    }
    Ok(blockers)
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn decode_repository(row: &crate::db::DbRow) -> Result<RepositoryRow> {
    Ok(RepositoryRow {
        id: row.get_text(0)?,
        name: row.get_text(1)?,
        repo_url: row.get_text_opt(2)?,
        local_path: row.get_text(3)?,
        default_branch: row.get_text(4)?,
        created_at: row.get_i64(5)?,
        created_by: row.get_text_opt(6)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::adapter::DbAdapter;
    use crate::db::migrate::DialectAwareMigrationSource;
    use crate::db::pool::{DbBackend, DbPool};
    use sqlx::sqlite::SqlitePool;

    /// Build a fresh in-memory SQLite adapter with the portable migration
    /// set applied. Mirrors the helper in db/onboarding.rs and
    /// db/user_worktree_commands.rs tests.
    async fn fresh_adapter() -> DbAdapter {
        let pool = SqlitePool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        let source = DialectAwareMigrationSource::for_backend(DbBackend::Sqlite);
        sqlx::migrate::Migrator::new(source)
            .await
            .expect("build migrator")
            .run(&pool)
            .await
            .expect("run migrations");
        DbAdapter::new(DbPool::Sqlite(pool))
    }

    /// Seed a user row directly via the adapter — the users DAO is still on
    /// the legacy rusqlite path; this lets us bypass it in tests.
    async fn seed_user(adapter: &DbAdapter, username: &str, role: &str) -> String {
        let id = format!("u-{username}");
        adapter
            .execute(
                "INSERT INTO users (id, username, role) VALUES (?, ?, ?)",
                vec![
                    DbValue::Text(id.clone()),
                    DbValue::Text(username.to_string()),
                    DbValue::Text(role.to_string()),
                ],
            )
            .await
            .expect("seed user");
        id
    }

    #[tokio::test]
    async fn upsert_creates_then_returns_same_id_for_existing_path() {
        let a = fresh_adapter().await;
        let id1 = upsert(
            &a,
            "takuto-core",
            Some("https://github.com/owner/takuto-core"),
            "/workspaces/takuto-core",
            "main",
            None,
        )
        .await
        .unwrap();
        // Second call with same local_path returns the same id; "name" /
        // "repo_url" updates are not applied (upsert is insert-if-missing).
        let id2 = upsert(
            &a,
            "different-name",
            Some("https://github.com/other/repo"),
            "/workspaces/takuto-core",
            "trunk",
            None,
        )
        .await
        .unwrap();
        assert_eq!(id1, id2, "upsert must be idempotent on local_path");

        let row = get(&a, &id1).await.unwrap().unwrap();
        assert_eq!(row.name, "takuto-core", "original name preserved");
        assert_eq!(
            row.repo_url,
            Some("https://github.com/owner/takuto-core".to_string()),
            "original url preserved"
        );
        assert_eq!(row.default_branch, "main", "original branch preserved");
        assert!(row.created_at > 0);
    }

    #[tokio::test]
    async fn upsert_allows_duplicate_names_on_different_paths() {
        let a = fresh_adapter().await;
        let id_a = upsert(
            &a,
            "foo",
            Some("https://github.com/owner-a/foo"),
            "/workspaces/foo",
            "main",
            None,
        )
        .await
        .unwrap();
        // Same `name` but different `local_path` — must coexist (no UNIQUE on name).
        let id_b = upsert(
            &a,
            "foo",
            Some("https://github.com/owner-b/foo"),
            "/workspaces/foo-2",
            "main",
            None,
        )
        .await
        .unwrap();
        assert_ne!(id_a, id_b);

        let all = list_all(&a).await.unwrap();
        assert_eq!(all.len(), 2);
        // Both have `name = "foo"`.
        assert!(all.iter().all(|r| r.name == "foo"));
    }

    #[tokio::test]
    async fn get_by_name_and_path_lookups_work() {
        let a = fresh_adapter().await;
        let id = upsert(
            &a,
            "takuto-core",
            None,
            "/workspaces/takuto-core",
            "main",
            None,
        )
        .await
        .unwrap();

        let by_name = get_by_name(&a, "takuto-core").await.unwrap().unwrap();
        assert_eq!(by_name.id, id);

        let by_path = get_by_path(&a, "/workspaces/takuto-core")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(by_path.id, id);

        assert!(get_by_name(&a, "does-not-exist").await.unwrap().is_none());
        assert!(get_by_path(&a, "/nope").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn delete_removes_row_and_cascades_to_user_repositories() {
        let a = fresh_adapter().await;
        let alice = seed_user(&a, "alice", "user").await;
        let bob = seed_user(&a, "bob", "user").await;
        let repo = upsert(
            &a,
            "takuto-core",
            None,
            "/workspaces/takuto-core",
            "main",
            None,
        )
        .await
        .unwrap();

        // Both users add the repo.
        assert!(add_for_user(&a, &alice, &repo).await.unwrap());
        assert!(add_for_user(&a, &bob, &repo).await.unwrap());
        // Sanity: 2 association rows.
        let row = a
            .query_one("SELECT COUNT(*) FROM user_repositories", vec![])
            .await
            .unwrap();
        assert_eq!(row.get_i64(0).unwrap(), 2);

        // Delete the repo → cascades to both association rows.
        assert!(delete(&a, &repo).await.unwrap());
        let row = a
            .query_one("SELECT COUNT(*) FROM user_repositories", vec![])
            .await
            .unwrap();
        assert_eq!(
            row.get_i64(0).unwrap(),
            0,
            "FK cascade must drop association rows"
        );

        // Second delete returns false.
        assert!(!delete(&a, &repo).await.unwrap());
    }

    #[tokio::test]
    async fn user_delete_cascades_to_user_repositories() {
        let a = fresh_adapter().await;
        let alice = seed_user(&a, "alice", "admin").await;
        let _bob = seed_user(&a, "bob", "admin").await;
        let repo = upsert(
            &a,
            "takuto-core",
            None,
            "/workspaces/takuto-core",
            "main",
            None,
        )
        .await
        .unwrap();
        add_for_user(&a, &alice, &repo).await.unwrap();

        // Direct DELETE on users bypasses the users DAO (still rusqlite) —
        // we exercise the ON DELETE CASCADE on user_repositories.user_id.
        a.execute(
            "DELETE FROM users WHERE id = ?",
            vec![DbValue::Text(alice.clone())],
        )
        .await
        .unwrap();

        let row = a
            .query_one(
                "SELECT COUNT(*) FROM user_repositories WHERE user_id = ?",
                vec![DbValue::Text(alice.clone())],
            )
            .await
            .unwrap();
        assert_eq!(row.get_i64(0).unwrap(), 0);
        // The repository row itself is preserved.
        assert!(get(&a, &repo).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn add_for_user_returns_true_then_false_on_duplicate() {
        let a = fresh_adapter().await;
        let alice = seed_user(&a, "alice", "user").await;
        let repo = upsert(&a, "x", None, "/workspaces/x", "main", None)
            .await
            .unwrap();

        assert!(add_for_user(&a, &alice, &repo).await.unwrap());
        // Second add for the same pair is a no-op.
        assert!(!add_for_user(&a, &alice, &repo).await.unwrap());
    }

    #[tokio::test]
    async fn remove_for_user_returns_true_then_false() {
        let a = fresh_adapter().await;
        let alice = seed_user(&a, "alice", "user").await;
        let repo = upsert(&a, "x", None, "/workspaces/x", "main", None)
            .await
            .unwrap();
        add_for_user(&a, &alice, &repo).await.unwrap();

        assert!(remove_for_user(&a, &alice, &repo).await.unwrap());
        assert!(!remove_for_user(&a, &alice, &repo).await.unwrap());
    }

    #[tokio::test]
    async fn list_for_user_returns_only_my_rows() {
        let a = fresh_adapter().await;
        let alice = seed_user(&a, "alice", "user").await;
        let bob = seed_user(&a, "bob", "user").await;
        let r1 = upsert(&a, "r1", None, "/workspaces/r1", "main", None)
            .await
            .unwrap();
        let r2 = upsert(&a, "r2", None, "/workspaces/r2", "main", None)
            .await
            .unwrap();
        let r3 = upsert(&a, "r3", None, "/workspaces/r3", "main", None)
            .await
            .unwrap();

        add_for_user(&a, &alice, &r1).await.unwrap();
        add_for_user(&a, &alice, &r2).await.unwrap();
        add_for_user(&a, &bob, &r3).await.unwrap();

        let alice_rows = list_for_user(&a, &alice).await.unwrap();
        assert_eq!(alice_rows.len(), 2);
        let names: std::collections::BTreeSet<&str> =
            alice_rows.iter().map(|r| r.name.as_str()).collect();
        // The primary sort is `added_at DESC`; r1 and r2 were inserted
        // within the same second, so the order between them is the
        // secondary sort (`ur.repository_id DESC` — alphanumeric on
        // random UUIDs, NOT insertion order). Assert membership instead
        // of position. Sub-second insertion-order determinism is a
        // documented non-feature across the cross-backend cluster.
        assert!(names.contains("r1"), "alice must see r1, got: {names:?}");
        assert!(names.contains("r2"), "alice must see r2, got: {names:?}");

        let bob_rows = list_for_user(&a, &bob).await.unwrap();
        assert_eq!(bob_rows.len(), 1);
        assert_eq!(bob_rows[0].name, "r3");
    }

    #[tokio::test]
    async fn list_available_for_user_is_complement_of_list_for_user() {
        let a = fresh_adapter().await;
        let alice = seed_user(&a, "alice", "user").await;
        let r1 = upsert(&a, "r1", None, "/workspaces/r1", "main", None)
            .await
            .unwrap();
        let r2 = upsert(&a, "r2", None, "/workspaces/r2", "main", None)
            .await
            .unwrap();
        let r3 = upsert(&a, "r3", None, "/workspaces/r3", "main", None)
            .await
            .unwrap();

        add_for_user(&a, &alice, &r1).await.unwrap();

        let avail = list_available_for_user(&a, &alice).await.unwrap();
        let avail_ids: Vec<&str> = avail.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(avail.len(), 2);
        assert!(avail_ids.contains(&r2.as_str()));
        assert!(avail_ids.contains(&r3.as_str()));
        assert!(!avail_ids.contains(&r1.as_str()));
    }

    #[tokio::test]
    async fn user_has_returns_correct_membership() {
        let a = fresh_adapter().await;
        let alice = seed_user(&a, "alice", "user").await;
        let bob = seed_user(&a, "bob", "user").await;
        let repo = upsert(
            &a,
            "takuto-core",
            None,
            "/workspaces/takuto-core",
            "main",
            None,
        )
        .await
        .unwrap();
        add_for_user(&a, &alice, &repo).await.unwrap();

        assert!(user_has(&a, &alice, "takuto-core").await.unwrap());
        assert!(!user_has(&a, &bob, "takuto-core").await.unwrap());
        assert!(!user_has(&a, &alice, "does-not-exist").await.unwrap());
    }

    #[tokio::test]
    async fn user_has_works_when_two_repos_share_a_name() {
        let a = fresh_adapter().await;
        let alice = seed_user(&a, "alice", "user").await;
        let foo_a = upsert(
            &a,
            "foo",
            Some("https://github.com/owner-a/foo"),
            "/workspaces/foo",
            "main",
            None,
        )
        .await
        .unwrap();
        let _foo_b = upsert(
            &a,
            "foo",
            Some("https://github.com/owner-b/foo"),
            "/workspaces/foo-2",
            "main",
            None,
        )
        .await
        .unwrap();
        add_for_user(&a, &alice, &foo_a).await.unwrap();

        assert!(user_has(&a, &alice, "foo").await.unwrap());
    }

    #[tokio::test]
    async fn list_all_returns_every_repository() {
        let a = fresh_adapter().await;
        upsert(&a, "r1", None, "/workspaces/r1", "main", None)
            .await
            .unwrap();
        upsert(&a, "r2", None, "/workspaces/r2", "main", None)
            .await
            .unwrap();
        upsert(&a, "r3", None, "/workspaces/r3", "main", None)
            .await
            .unwrap();

        let all = list_all(&a).await.unwrap();
        assert_eq!(all.len(), 3);
    }

    #[tokio::test]
    async fn created_by_set_null_on_user_delete() {
        // `repositories.created_by → users(id) ON DELETE SET NULL` —
        // deleting the user who registered a repo keeps the repo row but
        // nulls out the `created_by` field.
        let a = fresh_adapter().await;
        let alice = seed_user(&a, "alice", "admin").await;
        let _bob = seed_user(&a, "bob", "admin").await;
        let repo = upsert(
            &a,
            "takuto-core",
            None,
            "/workspaces/takuto-core",
            "main",
            Some(&alice),
        )
        .await
        .unwrap();

        a.execute(
            "DELETE FROM users WHERE id = ?",
            vec![DbValue::Text(alice.clone())],
        )
        .await
        .unwrap();

        let row = get(&a, &repo).await.unwrap().unwrap();
        assert_eq!(row.created_by, None, "FK SET NULL on user delete");
    }

    #[tokio::test]
    async fn upsert_with_no_repo_url_and_no_creator() {
        let a = fresh_adapter().await;
        let id = upsert(&a, "orphan", None, "/workspaces/orphan", "main", None)
            .await
            .unwrap();
        let row = get(&a, &id).await.unwrap().unwrap();
        assert_eq!(row.repo_url, None);
        assert_eq!(row.created_by, None);
    }

    /// `repository_has_active_workflow` returns `(ticket_key, user_id)` pairs
    /// when there are active workflows referencing the repo's `name`. With no
    /// snapshots on disk (in-memory test, no `TAKUTO_DATA_DIR` pointing at
    /// data), it returns an empty list — that's the contract.
    #[tokio::test]
    async fn repository_has_active_workflow_empty_without_snapshots() {
        let a = fresh_adapter().await;
        let repo = upsert(&a, "x", None, "/workspaces/x", "main", None)
            .await
            .unwrap();
        let blockers = repository_has_active_workflow(&a, &repo).await.unwrap();
        assert!(blockers.is_empty());
    }

    /// Even with no data dir resolved at all (no env), the helper must not
    /// panic — it returns an empty list.
    #[tokio::test]
    async fn repository_has_active_workflow_unknown_id_is_empty() {
        let a = fresh_adapter().await;
        let blockers = repository_has_active_workflow(&a, "no-such-id")
            .await
            .unwrap();
        assert!(blockers.is_empty());
    }
}
