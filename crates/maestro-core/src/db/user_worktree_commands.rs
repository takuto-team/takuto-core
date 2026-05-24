// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Per-user-per-workspace worktree settings (plan-09).
//!
//! Replaces plan-08's admin-scoped `workspace_commands` with a user-scoped
//! table keyed by `(user_id, workspace_name)`. Each row holds BOTH command
//! kinds in one record:
//!
//! - `init_commands_json` — a JSON array of strings. Each entry is a single
//!   `bash -lc` invocation run during worktree bootstrap.
//! - `run_commands_json` — a JSON array of `{name, command}` objects. Each
//!   entry becomes a button on the workflow card (e.g. "Run Dashboard UI",
//!   "Run Storybook").
//!
//! Two JSON columns rather than two tables: a single round-trip per
//! `(user, workspace)` lookup, atomic updates, and fewer endpoints. The
//! application layer knows the schema for each column.
//!
//! `user_id` references `users(id) ON DELETE CASCADE` — removing a user wipes
//! every row they configured (plan-09 AC-7).

use std::collections::HashMap;

use rusqlite::{Connection, params, params_from_iter};
use serde::{Deserialize, Serialize};

use crate::error::{MaestroError, Result};

/// A single run-command entry. Surfaced on workflow cards as a button labelled
/// `name` that, when clicked, executes `command` inside the worktree.
///
/// `Serialize`/`Deserialize` are how rows round-trip through the
/// `run_commands_json` column. `Default` is convenient for tests and the
/// REST layer.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunCommand {
    pub name: String,
    pub command: String,
}

/// One per-user-per-workspace row. Aggregates both command kinds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserWorktreeCommandsRow {
    pub user_id: String,
    pub workspace_name: String,
    pub init_commands: Vec<String>,
    pub run_commands: Vec<RunCommand>,
    pub updated_at: i64,
}

/// Get the row for `(user_id, workspace_name)`, or `None` if no row exists.
///
/// JSON parse failures surface as `MaestroError::Database` — a corrupted row
/// is treated as a hard error here (unlike the batched lookup, which logs and
/// omits so a single bad row doesn't poison the whole dashboard).
pub fn get(
    conn: &Connection,
    user_id: &str,
    workspace_name: &str,
) -> Result<Option<UserWorktreeCommandsRow>> {
    let mut stmt = conn.prepare(
        "SELECT user_id, workspace_name, init_commands_json, run_commands_json, updated_at \
         FROM user_worktree_commands WHERE user_id = ?1 AND workspace_name = ?2",
    )?;

    // `row_to_user_worktree_commands` returns
    // `rusqlite::Result<Result<Row, MaestroError>>`. Mirror the layered-error
    // pattern from plan-08's `workspace_commands.rs`: `.optional()?` flips
    // `Result<T, rusqlite::Error>` into `Option<T>` over the same SQLite
    // error, then `Option::transpose` flips `Option<Result<_>>` into
    // `Result<Option<_>>` over JSON-parse errors.
    let row: Option<Result<UserWorktreeCommandsRow>> = stmt
        .query_row(
            params![user_id, workspace_name],
            row_to_user_worktree_commands,
        )
        .optional()?;

    row.transpose()
}

/// List every row owned by `user_id`, ordered by most-recently-updated first.
pub fn list_for_user(conn: &Connection, user_id: &str) -> Result<Vec<UserWorktreeCommandsRow>> {
    let mut stmt = conn.prepare(
        "SELECT user_id, workspace_name, init_commands_json, run_commands_json, updated_at \
         FROM user_worktree_commands WHERE user_id = ?1 ORDER BY updated_at DESC",
    )?;

    let rows = stmt.query_map(params![user_id], row_to_user_worktree_commands)?;

    let mut out = Vec::new();
    for r in rows {
        // Each row is a Result<Result<Row, MaestroError>, rusqlite::Error>.
        // Flatten so we surface either rusqlite or JSON errors.
        out.push(r??);
    }
    Ok(out)
}

/// Insert or replace the row for `(user_id, workspace_name)`.
///
/// Reject any string containing a `\0` byte (would silently truncate in
/// C-strings and several shell layers). The REST layer enforces all other
/// validation (list size, per-entry length, name/command non-empty, duplicate
/// run-command names) — this layer is the last-line guardrail against
/// physically corrupt data.
///
/// `updated_at` is set to `Utc::now().timestamp()`.
pub fn upsert(
    conn: &Connection,
    user_id: &str,
    workspace_name: &str,
    init_commands: &[String],
    run_commands: &[RunCommand],
) -> Result<()> {
    // NUL-byte guard across every string value we're about to persist.
    if user_id.contains('\0') || workspace_name.contains('\0') {
        return Err(MaestroError::DatabaseStr(
            "user_id or workspace_name contains a NUL byte".into(),
        ));
    }
    for cmd in init_commands {
        if cmd.contains('\0') {
            return Err(MaestroError::DatabaseStr(
                "init command contains a NUL byte".into(),
            ));
        }
    }
    for rc in run_commands {
        if rc.name.contains('\0') || rc.command.contains('\0') {
            return Err(MaestroError::DatabaseStr(
                "run command name or command contains a NUL byte".into(),
            ));
        }
    }

    let init_json = serde_json::to_string(init_commands)
        .map_err(|e| MaestroError::DatabaseStr(format!("serializing init_commands: {e}")))?;
    let run_json = serde_json::to_string(run_commands)
        .map_err(|e| MaestroError::DatabaseStr(format!("serializing run_commands: {e}")))?;
    let now = chrono::Utc::now().timestamp();

    conn.execute(
        "INSERT INTO user_worktree_commands \
            (user_id, workspace_name, init_commands_json, run_commands_json, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(user_id, workspace_name) DO UPDATE SET \
           init_commands_json = excluded.init_commands_json, \
           run_commands_json  = excluded.run_commands_json, \
           updated_at         = excluded.updated_at",
        params![user_id, workspace_name, init_json, run_json, now],
    )?;
    Ok(())
}

/// Remove the row for `(user_id, workspace_name)`. Returns `true` if a row
/// was deleted, `false` if none existed.
pub fn delete(conn: &Connection, user_id: &str, workspace_name: &str) -> Result<bool> {
    let affected = conn.execute(
        "DELETE FROM user_worktree_commands WHERE user_id = ?1 AND workspace_name = ?2",
        params![user_id, workspace_name],
    )?;
    Ok(affected > 0)
}

/// Batched lookup of `run_commands` for many `(user_id, workspace_name)`
/// pairs in a single query.
///
/// The workflows-list endpoint calls this once per request, passing the
/// unique `(user_id, workspace_name)` pairs across every workflow on the
/// dashboard — so a 50-workflow dashboard is one DB hop, not 50.
///
/// Returns a `HashMap` keyed by `(user_id, workspace_name)`:
/// - Empty input → empty map (no query issued).
/// - Pairs with no row → not in the map (caller treats as "no run commands").
/// - JSON parse failures → logged at warn and omitted (a corrupt row doesn't
///   break the rest of the dashboard).
pub fn get_run_commands_for_pairs(
    conn: &Connection,
    pairs: &[(&str, &str)],
) -> Result<HashMap<(String, String), Vec<RunCommand>>> {
    if pairs.is_empty() {
        return Ok(HashMap::new());
    }

    // Build `(user_id = ? AND workspace_name = ?) OR ...`. Portable across
    // SQLite versions and clearer than the row-value `IN ((?,?),...)` form
    // (which only works on 3.34+). Each pair contributes two positional
    // parameters in order.
    let clauses: Vec<&str> = pairs
        .iter()
        .map(|_| "(user_id = ? AND workspace_name = ?)")
        .collect();
    let sql = format!(
        "SELECT user_id, workspace_name, run_commands_json \
         FROM user_worktree_commands WHERE {}",
        clauses.join(" OR ")
    );

    let mut bind: Vec<&str> = Vec::with_capacity(pairs.len() * 2);
    for (uid, ws) in pairs {
        bind.push(uid);
        bind.push(ws);
    }

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(bind.iter()), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;

    let mut out: HashMap<(String, String), Vec<RunCommand>> = HashMap::new();
    for r in rows {
        let (uid, ws, run_json) = r?;
        match serde_json::from_str::<Vec<RunCommand>>(&run_json) {
            Ok(v) => {
                out.insert((uid, ws), v);
            }
            Err(e) => {
                // Don't fail the dashboard for one corrupt row — log and omit.
                tracing::warn!(
                    user_id = %uid,
                    workspace_name = %ws,
                    error = %e,
                    "skipping run_commands_json: deserialization failed"
                );
            }
        }
    }
    Ok(out)
}

/// Convert a row into a `UserWorktreeCommandsRow`, deserializing both JSON
/// columns.
///
/// Returns `rusqlite::Result<Result<UserWorktreeCommandsRow, MaestroError>>`
/// — outer = SQLite errors, inner = JSON errors. Callers flatten with `??`.
fn row_to_user_worktree_commands(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<Result<UserWorktreeCommandsRow>> {
    let user_id: String = row.get(0)?;
    let workspace_name: String = row.get(1)?;
    let init_json: String = row.get(2)?;
    let run_json: String = row.get(3)?;
    let updated_at: i64 = row.get(4)?;

    let parsed: Result<(Vec<String>, Vec<RunCommand>)> = (|| {
        let init = serde_json::from_str::<Vec<String>>(&init_json).map_err(|e| {
            MaestroError::DatabaseStr(format!(
                "decoding init_commands_json for ({user_id},{workspace_name}): {e}"
            ))
        })?;
        let run = serde_json::from_str::<Vec<RunCommand>>(&run_json).map_err(|e| {
            MaestroError::DatabaseStr(format!(
                "decoding run_commands_json for ({user_id},{workspace_name}): {e}"
            ))
        })?;
        Ok((init, run))
    })();

    Ok(parsed.map(|(init_commands, run_commands)| UserWorktreeCommandsRow {
        user_id,
        workspace_name,
        init_commands,
        run_commands,
        updated_at,
    }))
}

/// Extension trait for optional query results. Mirrors the local helper in
/// `db/users.rs` and plan-08's `workspace_commands.rs` — keeping the shape
/// localized avoids a cross-module dependency on a tiny adapter.
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
    fn get_returns_none_on_missing() {
        let conn = test_conn();
        let alice = make_user(&conn, "alice");
        assert!(get(&conn, &alice, "frontend").unwrap().is_none());
    }

    #[test]
    fn upsert_then_get_roundtrips_both_kinds_in_order() {
        let conn = test_conn();
        let alice = make_user(&conn, "alice");

        let init = vec![
            "cd ui && npm install --legacy-peer-deps".to_string(),
            "cargo build".to_string(),
        ];
        let run = vec![
            RunCommand {
                name: "Dashboard UI".to_string(),
                command: "cd ui && npm run dev".to_string(),
            },
            RunCommand {
                name: "Storybook".to_string(),
                command: "cd ui && npm run storybook".to_string(),
            },
        ];

        upsert(&conn, &alice, "frontend", &init, &run).unwrap();

        let row = get(&conn, &alice, "frontend").unwrap().unwrap();
        assert_eq!(row.user_id, alice);
        assert_eq!(row.workspace_name, "frontend");
        assert_eq!(row.init_commands, init, "init order must be preserved");
        assert_eq!(row.run_commands, run, "run order must be preserved");
        assert!(row.updated_at > 0);
    }

    #[test]
    fn upsert_updates_updated_at_on_overwrite() {
        let conn = test_conn();
        let alice = make_user(&conn, "alice");

        upsert(&conn, &alice, "frontend", &["echo first".to_string()], &[]).unwrap();
        let first = get(&conn, &alice, "frontend").unwrap().unwrap();

        // Force the recorded timestamp backward so the next upsert is
        // guaranteed strictly greater than the prior value, even on fast
        // machines where the timestamp would otherwise tick once per second.
        conn.execute(
            "UPDATE user_worktree_commands SET updated_at = ?1 \
             WHERE user_id = ?2 AND workspace_name = 'frontend'",
            params![first.updated_at - 100, alice],
        )
        .unwrap();

        upsert(
            &conn,
            &alice,
            "frontend",
            &["echo second".to_string()],
            &[RunCommand {
                name: "Dev".to_string(),
                command: "npm run dev".to_string(),
            }],
        )
        .unwrap();
        let second = get(&conn, &alice, "frontend").unwrap().unwrap();

        assert!(
            second.updated_at > first.updated_at - 100,
            "updated_at should have moved forward; before={}, after={}",
            first.updated_at - 100,
            second.updated_at
        );
        assert_eq!(second.init_commands, vec!["echo second".to_string()]);
        assert_eq!(second.run_commands.len(), 1);
        assert_eq!(second.run_commands[0].name, "Dev");
    }

    #[test]
    fn delete_returns_true_then_false() {
        let conn = test_conn();
        let alice = make_user(&conn, "alice");
        upsert(&conn, &alice, "frontend", &["echo hi".to_string()], &[]).unwrap();

        assert!(delete(&conn, &alice, "frontend").unwrap());
        assert!(!delete(&conn, &alice, "frontend").unwrap());
        assert!(get(&conn, &alice, "frontend").unwrap().is_none());
    }

    #[test]
    fn list_for_user_returns_only_their_rows() {
        let conn = test_conn();
        let alice = make_user(&conn, "alice");
        let bob = make_user(&conn, "bob");

        upsert(&conn, &alice, "frontend", &["a".to_string()], &[]).unwrap();
        upsert(&conn, &alice, "backend", &["b".to_string()], &[]).unwrap();
        upsert(&conn, &bob, "frontend", &["b-frontend".to_string()], &[]).unwrap();

        let alice_rows = list_for_user(&conn, &alice).unwrap();
        assert_eq!(alice_rows.len(), 2);
        for r in &alice_rows {
            assert_eq!(r.user_id, alice, "isolation: alice sees only her rows");
        }

        let bob_rows = list_for_user(&conn, &bob).unwrap();
        assert_eq!(bob_rows.len(), 1);
        assert_eq!(bob_rows[0].workspace_name, "frontend");
        assert_eq!(bob_rows[0].init_commands, vec!["b-frontend".to_string()]);
    }

    #[test]
    fn list_for_user_orders_by_updated_at_desc() {
        let conn = test_conn();
        let alice = make_user(&conn, "alice");

        upsert(&conn, &alice, "first", &["a".to_string()], &[]).unwrap();
        upsert(&conn, &alice, "second", &["b".to_string()], &[]).unwrap();
        upsert(&conn, &alice, "third", &["c".to_string()], &[]).unwrap();

        // Force a deterministic ordering: third > second > first.
        conn.execute(
            "UPDATE user_worktree_commands SET updated_at = 300 \
             WHERE user_id = ?1 AND workspace_name = 'third'",
            params![alice],
        )
        .unwrap();
        conn.execute(
            "UPDATE user_worktree_commands SET updated_at = 200 \
             WHERE user_id = ?1 AND workspace_name = 'second'",
            params![alice],
        )
        .unwrap();
        conn.execute(
            "UPDATE user_worktree_commands SET updated_at = 100 \
             WHERE user_id = ?1 AND workspace_name = 'first'",
            params![alice],
        )
        .unwrap();

        let rows = list_for_user(&conn, &alice).unwrap();
        let names: Vec<&str> = rows.iter().map(|r| r.workspace_name.as_str()).collect();
        assert_eq!(names, vec!["third", "second", "first"]);
    }

    #[test]
    fn get_run_commands_for_pairs_batches_correctly() {
        let conn = test_conn();
        let alice = make_user(&conn, "alice");
        let bob = make_user(&conn, "bob");

        let run_a_frontend = vec![RunCommand {
            name: "Dashboard".to_string(),
            command: "npm run dev".to_string(),
        }];
        let run_b_frontend = vec![RunCommand {
            name: "Storybook".to_string(),
            command: "npm run sb".to_string(),
        }];

        upsert(&conn, &alice, "frontend", &[], &run_a_frontend).unwrap();
        upsert(&conn, &alice, "backend", &[], &[]).unwrap(); // empty run -> hit, empty vec
        upsert(&conn, &bob, "frontend", &[], &run_b_frontend).unwrap();

        // Mix of hits and misses.
        let alice_owned = alice.clone();
        let bob_owned = bob.clone();
        let pairs: Vec<(&str, &str)> = vec![
            (&alice_owned, "frontend"),       // hit, non-empty run
            (&alice_owned, "backend"),        // hit, empty run
            (&alice_owned, "does-not-exist"), // miss
            (&bob_owned, "frontend"),         // hit, non-empty run
            (&bob_owned, "backend"),          // miss (bob has no backend row)
        ];

        let map = get_run_commands_for_pairs(&conn, &pairs).unwrap();
        assert_eq!(map.len(), 3, "three hits expected, got: {map:?}");

        assert_eq!(
            map.get(&(alice_owned.clone(), "frontend".to_string())),
            Some(&run_a_frontend)
        );
        assert_eq!(
            map.get(&(alice_owned.clone(), "backend".to_string())),
            Some(&vec![])
        );
        assert_eq!(
            map.get(&(bob_owned.clone(), "frontend".to_string())),
            Some(&run_b_frontend)
        );
        // Misses are absent from the map (not present as empty vecs).
        assert!(
            !map.contains_key(&(alice_owned, "does-not-exist".to_string())),
            "miss should be absent, not empty"
        );
        assert!(!map.contains_key(&(bob_owned, "backend".to_string())));
    }

    #[test]
    fn get_run_commands_for_pairs_empty_input_returns_empty_map() {
        let conn = test_conn();
        let map = get_run_commands_for_pairs(&conn, &[]).unwrap();
        assert!(map.is_empty());
    }

    #[test]
    fn nul_byte_in_command_is_rejected() {
        let conn = test_conn();
        let alice = make_user(&conn, "alice");

        // NUL in an init command.
        let err = upsert(
            &conn,
            &alice,
            "frontend",
            &["foo\0bar".to_string()],
            &[],
        );
        assert!(err.is_err(), "NUL in init command must be rejected");

        // NUL in a run-command name.
        let err = upsert(
            &conn,
            &alice,
            "frontend",
            &[],
            &[RunCommand {
                name: "bad\0name".to_string(),
                command: "echo".to_string(),
            }],
        );
        assert!(err.is_err(), "NUL in run-command name must be rejected");

        // NUL in a run-command command.
        let err = upsert(
            &conn,
            &alice,
            "frontend",
            &[],
            &[RunCommand {
                name: "ok".to_string(),
                command: "echo \0".to_string(),
            }],
        );
        assert!(err.is_err(), "NUL in run-command command must be rejected");

        // NUL in workspace_name.
        let err = upsert(&conn, &alice, "fro\0nt", &[], &[]);
        assert!(err.is_err(), "NUL in workspace_name must be rejected");

        // No row should have been written by any of the failures above.
        assert!(get(&conn, &alice, "frontend").unwrap().is_none());
    }

    /// AC-7: deleting a user cascades to their `user_worktree_commands` rows.
    #[test]
    fn fk_cascade_deletes_rows_on_user_delete() {
        let conn = test_conn();
        // Need at least two admins so the "last admin" guard in `delete_user`
        // doesn't kick in if our user happens to be admin.
        let alice = create_user(&conn, "alice", UserRole::Admin).unwrap().id;
        let _bob = create_user(&conn, "bob", UserRole::Admin).unwrap().id;

        upsert(
            &conn,
            &alice,
            "frontend",
            &["echo hi".to_string()],
            &[RunCommand {
                name: "Dev".to_string(),
                command: "npm run dev".to_string(),
            }],
        )
        .unwrap();
        upsert(&conn, &alice, "backend", &["echo b".to_string()], &[]).unwrap();

        // Sanity: alice has 2 rows.
        assert_eq!(list_for_user(&conn, &alice).unwrap().len(), 2);

        crate::db::users::delete_user(&conn, &alice).unwrap();

        // After cascade, alice has 0 rows.
        assert_eq!(list_for_user(&conn, &alice).unwrap().len(), 0);
        // Direct count, too — guards against accidentally hiding via WHERE
        // user_id filter.
        let total: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM user_worktree_commands",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(total, 0, "FK cascade must drop every row alice owned");
    }

    /// JSON shape sanity check: init = list of strings, run = list of objects
    /// with `name` + `command`. Asserted by reading the raw JSON columns and
    /// re-parsing them with a strict typed schema.
    #[test]
    fn json_shapes_round_trip_correctly() {
        let conn = test_conn();
        let alice = make_user(&conn, "alice");

        upsert(
            &conn,
            &alice,
            "frontend",
            &["echo a".to_string(), "echo b".to_string()],
            &[RunCommand {
                name: "Dev".to_string(),
                command: "npm run dev".to_string(),
            }],
        )
        .unwrap();

        let init_raw: String = conn
            .query_row(
                "SELECT init_commands_json FROM user_worktree_commands \
                 WHERE user_id = ?1 AND workspace_name = 'frontend'",
                params![alice],
                |r| r.get(0),
            )
            .unwrap();
        let run_raw: String = conn
            .query_row(
                "SELECT run_commands_json FROM user_worktree_commands \
                 WHERE user_id = ?1 AND workspace_name = 'frontend'",
                params![alice],
                |r| r.get(0),
            )
            .unwrap();

        // init: pure list of strings.
        let init_parsed: Vec<String> = serde_json::from_str(&init_raw).unwrap();
        assert_eq!(init_parsed, vec!["echo a".to_string(), "echo b".to_string()]);

        // run: list of objects with `name` and `command` fields.
        let run_value: serde_json::Value = serde_json::from_str(&run_raw).unwrap();
        let arr = run_value.as_array().expect("run column must be an array");
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["name"], "Dev");
        assert_eq!(arr[0]["command"], "npm run dev");
    }

    /// Inserting a row referencing a non-existent user is rejected by the FK.
    /// Belt-and-suspenders for the cascade test above.
    #[test]
    fn fk_rejects_unknown_user_id() {
        let conn = test_conn();
        // No user created — direct upsert should fail at the FK.
        let res = upsert(
            &conn,
            "no-such-user-id",
            "frontend",
            &[],
            &[],
        );
        assert!(res.is_err(), "FK should reject unknown user_id");
    }

    /// Empty init/run arrays round-trip as `[]` and parse back to empty Vecs.
    #[test]
    fn upsert_accepts_both_empty_arrays() {
        let conn = test_conn();
        let alice = make_user(&conn, "alice");
        upsert(&conn, &alice, "frontend", &[], &[]).unwrap();

        let row = get(&conn, &alice, "frontend").unwrap().unwrap();
        assert!(row.init_commands.is_empty());
        assert!(row.run_commands.is_empty());
    }
}
