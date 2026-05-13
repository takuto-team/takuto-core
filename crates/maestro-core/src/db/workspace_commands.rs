// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Per-workspace overrides for `[commands].worktree_init_commands` (plan-08).
//!
//! Each row stores an admin-authored command list (`Vec<String>`) keyed by
//! `workspace_name` — the last path component of `git.repo_path`, the same
//! identifier used by `snapshot.rs::workspace_name_from_repo_path`. The
//! engine's bootstrap path consults this table before falling back to the
//! global `Config.commands.worktree_init_commands`.
//!
//! The command array is stored as a JSON string in `commands_json` (sort order
//! matters and is intrinsic to the array; normalized rows would buy nothing
//! and cost join overhead). Each command is a single `bash -lc` invocation;
//! a `\n` inside a command means a multi-line shell script, not multiple
//! commands.
//!
//! `updated_by` references the writing admin's `users.id` with
//! `ON DELETE SET NULL` — if the admin is deleted, the override row stays
//! (operators still need their commands) but the audit link is cleared.

use rusqlite::{Connection, params};

use crate::error::{MaestroError, Result};

/// One per-workspace override row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceCommandsRow {
    pub workspace_name: String,
    pub commands: Vec<String>,
    pub updated_at: i64,
    pub updated_by: Option<String>,
}

/// Get the override row for `workspace_name`, or `None` if no override exists.
pub fn get(conn: &Connection, workspace_name: &str) -> Result<Option<WorkspaceCommandsRow>> {
    let mut stmt = conn.prepare(
        "SELECT workspace_name, commands_json, updated_at, updated_by \
         FROM workspace_commands WHERE workspace_name = ?1",
    )?;

    // row_to_workspace_commands returns `rusqlite::Result<Result<Row, MaestroError>>`,
    // so after `.optional()?` we hold `Option<Result<Row, MaestroError>>`.
    // `Option::transpose` flips that to `Result<Option<Row>, MaestroError>` —
    // the shape our `Result<Option<_>>` signature expects.
    let row: Option<Result<WorkspaceCommandsRow>> = stmt
        .query_row(params![workspace_name], row_to_workspace_commands)
        .optional()?;

    row.transpose()
}

/// List every override row, ordered by most-recently-updated first.
pub fn list(conn: &Connection) -> Result<Vec<WorkspaceCommandsRow>> {
    let mut stmt = conn.prepare(
        "SELECT workspace_name, commands_json, updated_at, updated_by \
         FROM workspace_commands ORDER BY updated_at DESC",
    )?;

    let rows = stmt.query_map([], row_to_workspace_commands)?;

    let mut out = Vec::new();
    for r in rows {
        // Each row is a Result<Result<Row, MaestroError>, rusqlite::Error>.
        // Flatten so we surface either rusqlite or JSON deserialization errors.
        out.push(r??);
    }
    Ok(out)
}

/// Return just the `workspace_name` keys of every override row, for the UI's
/// `_workspaces` endpoint to intersect with the filesystem scan.
pub fn list_workspaces_with_overrides(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT workspace_name FROM workspace_commands")?;
    let names = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(names)
}

/// Insert or replace the override for `workspace_name`.
///
/// Reject any command string containing a `\0` byte (would silently truncate
/// in C-strings + several shell layers). Empty / trim / length validation is
/// the caller's job (REST layer enforces limits — list size, per-command
/// length, JSON ceiling).
pub fn upsert(
    conn: &Connection,
    workspace_name: &str,
    commands: &[String],
    updated_by: Option<&str>,
) -> Result<()> {
    for cmd in commands {
        if cmd.contains('\0') {
            return Err(MaestroError::Database(
                "command string contains a NUL byte".into(),
            ));
        }
    }

    let commands_json = serde_json::to_string(commands)
        .map_err(|e| MaestroError::Database(format!("serializing commands: {e}")))?;
    let now = chrono::Utc::now().timestamp();

    conn.execute(
        "INSERT INTO workspace_commands (workspace_name, commands_json, updated_at, updated_by) \
         VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(workspace_name) DO UPDATE SET \
           commands_json = excluded.commands_json, \
           updated_at = excluded.updated_at, \
           updated_by = excluded.updated_by",
        params![workspace_name, commands_json, now, updated_by],
    )?;
    Ok(())
}

/// Remove the override for `workspace_name`. Returns `true` if a row was
/// deleted, `false` if no override existed for that workspace.
pub fn delete(conn: &Connection, workspace_name: &str) -> Result<bool> {
    let affected = conn.execute(
        "DELETE FROM workspace_commands WHERE workspace_name = ?1",
        params![workspace_name],
    )?;
    Ok(affected > 0)
}

/// Convert a row into a `WorkspaceCommandsRow`, deserializing `commands_json`.
///
/// Returns `rusqlite::Result<Result<WorkspaceCommandsRow, MaestroError>>` — the
/// outer layer surfaces SQLite errors, the inner layer surfaces JSON-parse
/// errors. Callers flatten with `??`.
fn row_to_workspace_commands(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<Result<WorkspaceCommandsRow>> {
    let workspace_name: String = row.get(0)?;
    let commands_json: String = row.get(1)?;
    let updated_at: i64 = row.get(2)?;
    let updated_by: Option<String> = row.get(3)?;

    let parsed = serde_json::from_str::<Vec<String>>(&commands_json).map_err(|e| {
        MaestroError::Database(format!(
            "decoding commands_json for workspace '{workspace_name}': {e}"
        ))
    });

    Ok(parsed.map(|commands| WorkspaceCommandsRow {
        workspace_name,
        commands,
        updated_at,
        updated_by,
    }))
}

/// Extension trait for optional query results (mirrors the helper in
/// `db/users.rs` — kept local to avoid a cross-module dependency).
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

    /// Helper: create an admin user and return their id.
    fn make_admin(conn: &Connection, name: &str) -> String {
        create_user(conn, name, UserRole::Admin).unwrap().id
    }

    #[test]
    fn get_returns_none_on_missing() {
        let conn = test_conn();
        assert!(get(&conn, "frontend").unwrap().is_none());
    }

    #[test]
    fn upsert_then_get_roundtrips_commands_in_order() {
        let conn = test_conn();
        let admin = make_admin(&conn, "alice");
        let cmds = vec![
            "cd ui && npm install --legacy-peer-deps".to_string(),
            "cargo build".to_string(),
            "npx -y skills add foo --yes".to_string(),
        ];

        upsert(&conn, "frontend", &cmds, Some(&admin)).unwrap();

        let row = get(&conn, "frontend").unwrap().unwrap();
        assert_eq!(row.workspace_name, "frontend");
        assert_eq!(row.commands, cmds, "order must be preserved");
        assert_eq!(row.updated_by.as_deref(), Some(admin.as_str()));
        assert!(row.updated_at > 0);
    }

    #[test]
    fn upsert_updates_updated_at_on_overwrite() {
        let conn = test_conn();
        let admin = make_admin(&conn, "alice");

        upsert(
            &conn,
            "frontend",
            &["echo first".to_string()],
            Some(&admin),
        )
        .unwrap();
        let first = get(&conn, "frontend").unwrap().unwrap();

        // Force the recorded timestamp backward so the next upsert is
        // guaranteed strictly greater, even on fast machines.
        conn.execute(
            "UPDATE workspace_commands SET updated_at = ?1 WHERE workspace_name = 'frontend'",
            params![first.updated_at - 100],
        )
        .unwrap();

        upsert(
            &conn,
            "frontend",
            &["echo second".to_string()],
            Some(&admin),
        )
        .unwrap();
        let second = get(&conn, "frontend").unwrap().unwrap();

        assert!(
            second.updated_at > first.updated_at - 100,
            "updated_at should have moved forward: was {}, now {}",
            first.updated_at - 100,
            second.updated_at,
        );
        assert_eq!(second.commands, vec!["echo second".to_string()]);
    }

    #[test]
    fn delete_returns_true_then_false() {
        let conn = test_conn();
        upsert(&conn, "frontend", &["echo hi".to_string()], None).unwrap();

        assert!(delete(&conn, "frontend").unwrap());
        assert!(!delete(&conn, "frontend").unwrap());
        assert!(get(&conn, "frontend").unwrap().is_none());
    }

    #[test]
    fn list_returns_all_rows() {
        let conn = test_conn();
        upsert(&conn, "frontend", &["a".to_string()], None).unwrap();
        upsert(&conn, "backend", &["b".to_string(), "c".to_string()], None).unwrap();
        upsert(&conn, "infra", &[], None).unwrap();

        let mut rows = list(&conn).unwrap();
        assert_eq!(rows.len(), 3);

        // Order-independent equality of the (name, commands) pairs.
        rows.sort_by(|a, b| a.workspace_name.cmp(&b.workspace_name));
        assert_eq!(rows[0].workspace_name, "backend");
        assert_eq!(rows[0].commands, vec!["b".to_string(), "c".to_string()]);
        assert_eq!(rows[1].workspace_name, "frontend");
        assert_eq!(rows[1].commands, vec!["a".to_string()]);
        assert_eq!(rows[2].workspace_name, "infra");
        assert!(rows[2].commands.is_empty());
    }

    #[test]
    fn list_workspaces_with_overrides_returns_keys() {
        let conn = test_conn();
        upsert(&conn, "frontend", &["a".to_string()], None).unwrap();
        upsert(&conn, "backend", &["b".to_string()], None).unwrap();

        let mut names = list_workspaces_with_overrides(&conn).unwrap();
        names.sort();
        assert_eq!(names, vec!["backend".to_string(), "frontend".to_string()]);

        // Empty case.
        delete(&conn, "frontend").unwrap();
        delete(&conn, "backend").unwrap();
        assert!(list_workspaces_with_overrides(&conn).unwrap().is_empty());
    }

    /// FK contract: deleting the `updated_by` user must NULL out the
    /// `updated_by` column on every `workspace_commands` row that referenced
    /// them. The row itself stays (operators still need their commands).
    #[test]
    fn fk_cascade_sets_updated_by_to_null() {
        let conn = test_conn();
        let admin = make_admin(&conn, "alice");
        upsert(&conn, "frontend", &["echo hi".to_string()], Some(&admin)).unwrap();

        // Sanity check: updated_by is currently the admin's id.
        let before = get(&conn, "frontend").unwrap().unwrap();
        assert_eq!(before.updated_by.as_deref(), Some(admin.as_str()));

        // Need a second admin so the "last admin" guard doesn't reject the delete.
        let _second = make_admin(&conn, "bob");
        crate::db::users::delete_user(&conn, &admin).unwrap();

        // Row still exists, updated_by has been cleared.
        let after = get(&conn, "frontend").unwrap().unwrap();
        assert!(
            after.updated_by.is_none(),
            "expected updated_by to be NULL after user delete, got {:?}",
            after.updated_by
        );
        assert_eq!(after.commands, vec!["echo hi".to_string()]);
    }

    #[test]
    fn upsert_rejects_nul_byte_in_command() {
        let conn = test_conn();
        let err = upsert(&conn, "frontend", &["foo\0bar".to_string()], None);
        assert!(
            err.is_err(),
            "expected NUL-byte command to be rejected, got Ok"
        );
        // No row should have been written.
        assert!(get(&conn, "frontend").unwrap().is_none());
    }

    #[test]
    fn upsert_accepts_empty_commands_array() {
        let conn = test_conn();
        upsert(&conn, "frontend", &[], None).unwrap();
        let row = get(&conn, "frontend").unwrap().unwrap();
        assert!(row.commands.is_empty());
    }

    #[test]
    fn upsert_accepts_multi_line_commands() {
        let conn = test_conn();
        let multi = "set -e\necho one\necho two".to_string();
        upsert(&conn, "frontend", &[multi.clone()], None).unwrap();
        let row = get(&conn, "frontend").unwrap().unwrap();
        assert_eq!(row.commands, vec![multi]);
    }
}
