// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Per-user repository registry (plan-10).
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
//! drive off this table. See `tmp/plan-10-per-user-repositories.md` for the
//! product model.

use rusqlite::{Connection, params};
use uuid::Uuid;

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
pub fn get(conn: &Connection, id: &str) -> Result<Option<RepositoryRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, repo_url, local_path, default_branch, created_at, created_by \
         FROM repositories WHERE id = ?1",
    )?;
    let row = stmt
        .query_row(params![id], row_to_repository)
        .optional()?;
    Ok(row)
}

/// Fetch a repository by `name`. Returns the first match — `name` is NOT
/// unique by design, so callers expecting ambiguity should use `list_all`.
/// Used by the workflow filter to map a snapshot's `workspace_name` →
/// `repository_id`.
pub fn get_by_name(conn: &Connection, name: &str) -> Result<Option<RepositoryRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, repo_url, local_path, default_branch, created_at, created_by \
         FROM repositories WHERE name = ?1 ORDER BY created_at ASC LIMIT 1",
    )?;
    let row = stmt
        .query_row(params![name], row_to_repository)
        .optional()?;
    Ok(row)
}

/// Fetch a repository by `local_path` (which is UNIQUE).
pub fn get_by_path(conn: &Connection, local_path: &str) -> Result<Option<RepositoryRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, repo_url, local_path, default_branch, created_at, created_by \
         FROM repositories WHERE local_path = ?1",
    )?;
    let row = stmt
        .query_row(params![local_path], row_to_repository)
        .optional()?;
    Ok(row)
}

/// List every registered repository, ordered by `created_at ASC, name ASC` for
/// determinism.
pub fn list_all(conn: &Connection) -> Result<Vec<RepositoryRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, repo_url, local_path, default_branch, created_at, created_by \
         FROM repositories ORDER BY created_at ASC, name ASC",
    )?;
    let rows = stmt
        .query_map([], row_to_repository)?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
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
/// change (none exists in plan-10).
pub fn upsert(
    conn: &Connection,
    name: &str,
    repo_url: Option<&str>,
    local_path: &str,
    default_branch: &str,
    created_by: Option<&str>,
) -> Result<String> {
    let new_id = Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp();

    conn.execute(
        "INSERT INTO repositories \
            (id, name, repo_url, local_path, default_branch, created_at, created_by) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
         ON CONFLICT(local_path) DO NOTHING",
        params![new_id, name, repo_url, local_path, default_branch, now, created_by],
    )?;

    // Read back the row's id — either the one we just inserted or the one
    // a racing peer inserted. `local_path` is UNIQUE so this query is
    // single-row.
    let id: String = conn.query_row(
        "SELECT id FROM repositories WHERE local_path = ?1",
        params![local_path],
        |r| r.get(0),
    )?;
    Ok(id)
}

/// Delete a `repositories` row by id. Returns `true` if a row was deleted,
/// `false` if no such id existed. Cascades to `user_repositories` via FK.
///
/// This is a DB-only delete; the on-disk clone is removed by the calling
/// REST handler when appropriate (see plan-10 Step 5 always-purge).
pub fn delete(conn: &Connection, id: &str) -> Result<bool> {
    let affected = conn.execute("DELETE FROM repositories WHERE id = ?1", params![id])?;
    Ok(affected > 0)
}

// ── user_repositories CRUD ──────────────────────────────────────────────────

/// List every repository the user has added, ordered by `added_at DESC` (most
/// recently added first — matches the start-manual default repo selection).
///
/// `added_at` is stored in whole seconds, so two calls to `add_for_user`
/// within the same second produce ties. Sub-second insertion order is
/// preserved via SQLite's monotonically-increasing `ROWID` as a secondary
/// sort key — the most recently inserted association still wins on ties.
pub fn list_for_user(conn: &Connection, user_id: &str) -> Result<Vec<RepositoryRow>> {
    let mut stmt = conn.prepare(
        "SELECT r.id, r.name, r.repo_url, r.local_path, r.default_branch, r.created_at, r.created_by \
         FROM repositories r \
         INNER JOIN user_repositories ur ON ur.repository_id = r.id \
         WHERE ur.user_id = ?1 \
         ORDER BY ur.added_at DESC, ur.rowid DESC",
    )?;
    let rows = stmt
        .query_map(params![user_id], row_to_repository)?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

/// List every registered repository the user has NOT yet added. Used by the
/// "Available repositories" picker in the My Repositories tab.
pub fn list_available_for_user(conn: &Connection, user_id: &str) -> Result<Vec<RepositoryRow>> {
    let mut stmt = conn.prepare(
        "SELECT r.id, r.name, r.repo_url, r.local_path, r.default_branch, r.created_at, r.created_by \
         FROM repositories r \
         WHERE NOT EXISTS ( \
             SELECT 1 FROM user_repositories ur \
             WHERE ur.repository_id = r.id AND ur.user_id = ?1 \
         ) \
         ORDER BY r.created_at ASC, r.name ASC",
    )?;
    let rows = stmt
        .query_map(params![user_id], row_to_repository)?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

/// Associate `repository_id` with `user_id`. Returns `true` when a new row was
/// inserted, `false` when the association already existed (idempotent).
///
/// Uses `INSERT ... ON CONFLICT(user_id, repository_id) DO NOTHING`, so racing
/// concurrent adds collapse to a single row.
pub fn add_for_user(
    conn: &Connection,
    user_id: &str,
    repository_id: &str,
) -> Result<bool> {
    let now = chrono::Utc::now().timestamp();
    let affected = conn.execute(
        "INSERT INTO user_repositories (user_id, repository_id, added_at) \
         VALUES (?1, ?2, ?3) \
         ON CONFLICT(user_id, repository_id) DO NOTHING",
        params![user_id, repository_id, now],
    )?;
    Ok(affected > 0)
}

/// Drop the association between `user_id` and `repository_id`. Returns `true`
/// when a row was deleted, `false` when no association existed.
///
/// This is a single-link delete only — it doesn't cascade to the
/// `repositories` row or the on-disk clone. The REST layer decides whether to
/// purge based on "last user remove" semantics (see plan-10 Step 5).
pub fn remove_for_user(
    conn: &Connection,
    user_id: &str,
    repository_id: &str,
) -> Result<bool> {
    let affected = conn.execute(
        "DELETE FROM user_repositories WHERE user_id = ?1 AND repository_id = ?2",
        params![user_id, repository_id],
    )?;
    Ok(affected > 0)
}

/// Does the user have any registered repository with this `name`?
///
/// Defensive back-compat path: workflow snapshots persist `workspace_name`
/// (a string) rather than `repository_id` for legacy rows. The Step 6 filter
/// uses this when the workflow's `repository_id` is absent. Returns `true` if
/// at least one repository with that `name` is in the user's added set.
pub fn user_has(
    conn: &Connection,
    user_id: &str,
    repository_name: &str,
) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM repositories r \
         INNER JOIN user_repositories ur ON ur.repository_id = r.id \
         WHERE ur.user_id = ?1 AND r.name = ?2",
        params![user_id, repository_name],
        |r| r.get(0),
    )?;
    Ok(count > 0)
}

/// Return every active (non-terminal) workflow on `repository_id`, as
/// `(ticket_key, user_id)` pairs. Used by the DELETE-repo handler to refuse
/// removal when active work is in progress (plan-10 AC-16).
///
/// "Active" = not Done, Stopped, or Error. We probe the per-workspace snapshot
/// files via `workspace_name` matching (durable for restored snapshots), then
/// filter in-memory by state. The snapshot row stores the workflow state as a
/// debug string (e.g. `"Done"`, `"Stopped"`, `"Error"`, `"Init"`, ...).
///
/// **Caveat**: at the time of writing, workflow snapshots persist by
/// `workspace_name` (the on-disk dir name) rather than `repository_id`. We
/// resolve `local_path → name → snapshot_dir` so this works for legacy and
/// post-plan-10 snapshots alike. If `repository_id` is later added to the
/// snapshot model, this helper should be updated to match on `repository_id`
/// first and fall back to `workspace_name`.
pub fn repository_has_active_workflow(
    conn: &Connection,
    repository_id: &str,
) -> Result<Vec<(String, String)>> {
    // 1. Resolve `repository_id` → `name` + `local_path`.
    let row = match get(conn, repository_id)? {
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
    let records = crate::workflow::snapshot::read_all_workspace_snapshots(&data_dir)
        .unwrap_or_default();

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

fn row_to_repository(row: &rusqlite::Row<'_>) -> rusqlite::Result<RepositoryRow> {
    Ok(RepositoryRow {
        id: row.get(0)?,
        name: row.get(1)?,
        repo_url: row.get(2)?,
        local_path: row.get(3)?,
        default_branch: row.get(4)?,
        created_at: row.get(5)?,
        created_by: row.get(6)?,
    })
}

/// Local optional-result adapter, mirroring the one in `db/users.rs` to keep
/// this module self-contained.
trait OptionalExt<T> {
    fn optional(self) -> rusqlite::Result<Option<T>>;
}

impl<T> OptionalExt<T> for rusqlite::Result<T> {
    fn optional(self) -> rusqlite::Result<Option<T>> {
        match self {
            Ok(val) => Ok(Some(val)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::UserRole;
    use crate::db::schema;
    use crate::db::users::create_user;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        schema::run_migrations(&conn).unwrap();
        conn
    }

    /// Helper: create a user and return their id.
    fn make_user(conn: &Connection, name: &str) -> String {
        create_user(conn, name, UserRole::User).unwrap().id
    }

    #[test]
    fn upsert_creates_then_returns_same_id_for_existing_path() {
        let conn = test_conn();
        let id1 = upsert(
            &conn,
            "maestro-core",
            Some("https://github.com/owner/maestro-core"),
            "/workspaces/maestro-core",
            "main",
            None,
        )
        .unwrap();
        // Second call with same local_path returns the same id; "name" /
        // "repo_url" updates are not applied (upsert is insert-if-missing).
        let id2 = upsert(
            &conn,
            "different-name",
            Some("https://github.com/other/repo"),
            "/workspaces/maestro-core",
            "trunk",
            None,
        )
        .unwrap();
        assert_eq!(id1, id2, "upsert must be idempotent on local_path");

        let row = get(&conn, &id1).unwrap().unwrap();
        assert_eq!(row.name, "maestro-core", "original name preserved");
        assert_eq!(
            row.repo_url,
            Some("https://github.com/owner/maestro-core".to_string()),
            "original url preserved"
        );
        assert_eq!(row.default_branch, "main", "original branch preserved");
        assert!(row.created_at > 0);
    }

    #[test]
    fn upsert_allows_duplicate_names_on_different_paths() {
        let conn = test_conn();
        let id_a = upsert(
            &conn,
            "foo",
            Some("https://github.com/owner-a/foo"),
            "/workspaces/foo",
            "main",
            None,
        )
        .unwrap();
        // Same `name` but different `local_path` — must coexist (no UNIQUE on name).
        let id_b = upsert(
            &conn,
            "foo",
            Some("https://github.com/owner-b/foo"),
            "/workspaces/foo-2",
            "main",
            None,
        )
        .unwrap();
        assert_ne!(id_a, id_b);

        let all = list_all(&conn).unwrap();
        assert_eq!(all.len(), 2);
        // Both have `name = "foo"`.
        assert!(all.iter().all(|r| r.name == "foo"));
    }

    #[test]
    fn get_by_name_and_path_lookups_work() {
        let conn = test_conn();
        let id = upsert(
            &conn,
            "maestro-core",
            None,
            "/workspaces/maestro-core",
            "main",
            None,
        )
        .unwrap();

        let by_name = get_by_name(&conn, "maestro-core").unwrap().unwrap();
        assert_eq!(by_name.id, id);

        let by_path = get_by_path(&conn, "/workspaces/maestro-core")
            .unwrap()
            .unwrap();
        assert_eq!(by_path.id, id);

        assert!(get_by_name(&conn, "does-not-exist").unwrap().is_none());
        assert!(get_by_path(&conn, "/nope").unwrap().is_none());
    }

    #[test]
    fn delete_removes_row_and_cascades_to_user_repositories() {
        let conn = test_conn();
        let alice = make_user(&conn, "alice");
        let bob = make_user(&conn, "bob");
        let repo = upsert(
            &conn,
            "maestro-core",
            None,
            "/workspaces/maestro-core",
            "main",
            None,
        )
        .unwrap();

        // Both users add the repo.
        assert!(add_for_user(&conn, &alice, &repo).unwrap());
        assert!(add_for_user(&conn, &bob, &repo).unwrap());
        // Sanity: 2 association rows.
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM user_repositories", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);

        // Delete the repo → cascades to both association rows.
        assert!(delete(&conn, &repo).unwrap());
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM user_repositories", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "FK cascade must drop association rows");

        // Second delete returns false.
        assert!(!delete(&conn, &repo).unwrap());
    }

    #[test]
    fn user_delete_cascades_to_user_repositories() {
        let conn = test_conn();
        // Two admins so the "last admin" guard doesn't block delete.
        let alice = create_user(&conn, "alice", UserRole::Admin).unwrap().id;
        let _bob = create_user(&conn, "bob", UserRole::Admin).unwrap().id;
        let repo = upsert(
            &conn,
            "maestro-core",
            None,
            "/workspaces/maestro-core",
            "main",
            None,
        )
        .unwrap();
        add_for_user(&conn, &alice, &repo).unwrap();

        crate::db::users::delete_user(&conn, &alice).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM user_repositories WHERE user_id = ?1",
                params![alice],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
        // The repository row itself is preserved.
        assert!(get(&conn, &repo).unwrap().is_some());
    }

    #[test]
    fn add_for_user_returns_true_then_false_on_duplicate() {
        let conn = test_conn();
        let alice = make_user(&conn, "alice");
        let repo = upsert(&conn, "x", None, "/workspaces/x", "main", None).unwrap();

        assert!(add_for_user(&conn, &alice, &repo).unwrap());
        // Second add for the same pair is a no-op.
        assert!(!add_for_user(&conn, &alice, &repo).unwrap());
    }

    #[test]
    fn remove_for_user_returns_true_then_false() {
        let conn = test_conn();
        let alice = make_user(&conn, "alice");
        let repo = upsert(&conn, "x", None, "/workspaces/x", "main", None).unwrap();
        add_for_user(&conn, &alice, &repo).unwrap();

        assert!(remove_for_user(&conn, &alice, &repo).unwrap());
        assert!(!remove_for_user(&conn, &alice, &repo).unwrap());
    }

    #[test]
    fn list_for_user_returns_only_my_rows() {
        let conn = test_conn();
        let alice = make_user(&conn, "alice");
        let bob = make_user(&conn, "bob");
        let r1 = upsert(&conn, "r1", None, "/workspaces/r1", "main", None).unwrap();
        let r2 = upsert(&conn, "r2", None, "/workspaces/r2", "main", None).unwrap();
        let r3 = upsert(&conn, "r3", None, "/workspaces/r3", "main", None).unwrap();

        add_for_user(&conn, &alice, &r1).unwrap();
        add_for_user(&conn, &alice, &r2).unwrap();
        add_for_user(&conn, &bob, &r3).unwrap();

        let alice_rows = list_for_user(&conn, &alice).unwrap();
        assert_eq!(alice_rows.len(), 2);
        let names: Vec<&str> = alice_rows.iter().map(|r| r.name.as_str()).collect();
        // Most recently added is first.
        assert_eq!(names[0], "r2");
        assert_eq!(names[1], "r1");

        let bob_rows = list_for_user(&conn, &bob).unwrap();
        assert_eq!(bob_rows.len(), 1);
        assert_eq!(bob_rows[0].name, "r3");
    }

    #[test]
    fn list_available_for_user_is_complement_of_list_for_user() {
        let conn = test_conn();
        let alice = make_user(&conn, "alice");
        let r1 = upsert(&conn, "r1", None, "/workspaces/r1", "main", None).unwrap();
        let r2 = upsert(&conn, "r2", None, "/workspaces/r2", "main", None).unwrap();
        let r3 = upsert(&conn, "r3", None, "/workspaces/r3", "main", None).unwrap();

        add_for_user(&conn, &alice, &r1).unwrap();

        let avail = list_available_for_user(&conn, &alice).unwrap();
        let avail_ids: Vec<&str> = avail.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(avail.len(), 2);
        assert!(avail_ids.contains(&r2.as_str()));
        assert!(avail_ids.contains(&r3.as_str()));
        assert!(!avail_ids.contains(&r1.as_str()));
    }

    #[test]
    fn user_has_returns_correct_membership() {
        let conn = test_conn();
        let alice = make_user(&conn, "alice");
        let bob = make_user(&conn, "bob");
        let repo = upsert(
            &conn,
            "maestro-core",
            None,
            "/workspaces/maestro-core",
            "main",
            None,
        )
        .unwrap();
        add_for_user(&conn, &alice, &repo).unwrap();

        assert!(user_has(&conn, &alice, "maestro-core").unwrap());
        assert!(!user_has(&conn, &bob, "maestro-core").unwrap());
        assert!(!user_has(&conn, &alice, "does-not-exist").unwrap());
    }

    #[test]
    fn user_has_works_when_two_repos_share_a_name() {
        // Two forks both named "foo"; alice has just one of them. `user_has`
        // returns true because at least one match is in her added set.
        let conn = test_conn();
        let alice = make_user(&conn, "alice");
        let foo_a = upsert(
            &conn,
            "foo",
            Some("https://github.com/owner-a/foo"),
            "/workspaces/foo",
            "main",
            None,
        )
        .unwrap();
        let _foo_b = upsert(
            &conn,
            "foo",
            Some("https://github.com/owner-b/foo"),
            "/workspaces/foo-2",
            "main",
            None,
        )
        .unwrap();
        add_for_user(&conn, &alice, &foo_a).unwrap();

        assert!(user_has(&conn, &alice, "foo").unwrap());
    }

    #[test]
    fn list_all_returns_every_repository() {
        let conn = test_conn();
        upsert(&conn, "r1", None, "/workspaces/r1", "main", None).unwrap();
        upsert(&conn, "r2", None, "/workspaces/r2", "main", None).unwrap();
        upsert(&conn, "r3", None, "/workspaces/r3", "main", None).unwrap();

        let all = list_all(&conn).unwrap();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn created_by_set_null_on_user_delete() {
        // `repositories.created_by → users(id) ON DELETE SET NULL` —
        // deleting the user who registered a repo keeps the repo row but
        // nulls out the `created_by` field.
        let conn = test_conn();
        let alice = create_user(&conn, "alice", UserRole::Admin).unwrap().id;
        let _bob = create_user(&conn, "bob", UserRole::Admin).unwrap().id;
        let repo = upsert(
            &conn,
            "maestro-core",
            None,
            "/workspaces/maestro-core",
            "main",
            Some(&alice),
        )
        .unwrap();

        crate::db::users::delete_user(&conn, &alice).unwrap();

        let row = get(&conn, &repo).unwrap().unwrap();
        assert_eq!(row.created_by, None, "FK SET NULL on user delete");
    }

    #[test]
    fn upsert_with_no_repo_url_and_no_creator() {
        let conn = test_conn();
        let id = upsert(
            &conn,
            "orphan",
            None,
            "/workspaces/orphan",
            "main",
            None,
        )
        .unwrap();
        let row = get(&conn, &id).unwrap().unwrap();
        assert_eq!(row.repo_url, None);
        assert_eq!(row.created_by, None);
    }

    /// `repository_has_active_workflow` returns `(ticket_key, user_id)` pairs
    /// when there are active workflows referencing the repo's `name`. With no
    /// snapshots on disk (in-memory test, no `MAESTRO_DATA_DIR` pointing at
    /// data), it returns an empty list — that's the contract.
    #[test]
    fn repository_has_active_workflow_empty_without_snapshots() {
        let conn = test_conn();
        let repo = upsert(&conn, "x", None, "/workspaces/x", "main", None).unwrap();
        let blockers = repository_has_active_workflow(&conn, &repo).unwrap();
        assert!(blockers.is_empty());
    }

    /// Even with no data dir resolved at all (no env), the helper must not
    /// panic — it returns an empty list.
    #[test]
    fn repository_has_active_workflow_unknown_id_is_empty() {
        let conn = test_conn();
        let blockers = repository_has_active_workflow(&conn, "no-such-id").unwrap();
        assert!(blockers.is_empty());
    }

}
