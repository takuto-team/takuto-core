// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Per-user-per-workspace worktree settings.
//!
//! User-scoped table keyed by `(user_id, workspace_name)`. Each row holds
//! BOTH command kinds in one record:
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
//! `user_id` references `users(id) ON DELETE CASCADE` — removing a user
//! wipes every row they configured.
//!
//! ### DAO conventions
//!
//! 1. **JSON columns**: encode/decode via `serde_json` at the DAO
//!    boundary. The on-disk format is TEXT with `[…]` JSON. Binds the
//!    encoded string via `DbValue::Text` and re-parses on read.
//!
//! 2. **Dynamic-arity bind**: `get_run_commands_for_pairs` accepts
//!    `&[(&str, &str)]` and builds N `?` placeholders. The bind loop
//!    walks the pairs vector — the adapter takes a `Vec<DbValue>` so we
//!    convert each `&str` to `DbValue::Text` and `extend` into the
//!    bind list.
//!
//! 3. **Hard vs soft JSON-decode failure**: `get` propagates a corrupt
//!    JSON as `DbError::CommandsJsonDecode` (envelope error); the batched
//!    `get_run_commands_for_pairs` logs at warn and omits the row so a
//!    single corrupt entry doesn't poison the whole dashboard.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::db::{DbAdapter, DbValue};
use crate::error::Result;

use super::DbError;

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
/// JSON parse failures surface as `DbError::CommandsJsonDecode` (envelope:
/// `MaestroError::Db`) — a corrupted row is treated as a hard error here
/// (unlike the batched lookup, which logs and omits so a single bad row
/// doesn't poison the whole dashboard).
pub async fn get(
    adapter: &DbAdapter,
    user_id: &str,
    workspace_name: &str,
) -> Result<Option<UserWorktreeCommandsRow>> {
    let row = adapter
        .query_optional(
            "SELECT user_id, workspace_name, init_commands_json, run_commands_json, updated_at \
             FROM user_worktree_commands WHERE user_id = ? AND workspace_name = ?",
            vec![
                DbValue::Text(user_id.to_string()),
                DbValue::Text(workspace_name.to_string()),
            ],
        )
        .await?;
    let Some(r) = row else {
        return Ok(None);
    };
    Ok(Some(decode_full_row(&r)?))
}

/// List every row owned by `user_id`, ordered by most-recently-updated first.
pub async fn list_for_user(
    adapter: &DbAdapter,
    user_id: &str,
) -> Result<Vec<UserWorktreeCommandsRow>> {
    let rows = adapter
        .query_all(
            "SELECT user_id, workspace_name, init_commands_json, run_commands_json, updated_at \
             FROM user_worktree_commands WHERE user_id = ? ORDER BY updated_at DESC",
            vec![DbValue::Text(user_id.to_string())],
        )
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        out.push(decode_full_row(r)?);
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
///
/// Backend support for the upsert clause: SQLite (≥ 3.24) and Postgres
/// (≥ 9.5) use the same `INSERT ... ON CONFLICT(...) DO UPDATE SET col =
/// excluded.col` form. MySQL uses `ON DUPLICATE KEY UPDATE` — when MySQL
/// is added as a supported backend, this fn will need a
/// `match adapter.backend()` branch.
pub async fn upsert(
    adapter: &DbAdapter,
    user_id: &str,
    workspace_name: &str,
    init_commands: &[String],
    run_commands: &[RunCommand],
) -> Result<()> {
    // NUL-byte guard across every string value we're about to persist.
    if user_id.contains('\0') || workspace_name.contains('\0') {
        return Err(DbError::NulByte {
            field: "user_id_or_workspace_name",
        }
        .into());
    }
    for cmd in init_commands {
        if cmd.contains('\0') {
            return Err(DbError::NulByte {
                field: "init_command",
            }
            .into());
        }
    }
    for rc in run_commands {
        if rc.name.contains('\0') || rc.command.contains('\0') {
            return Err(DbError::NulByte {
                field: "run_command_name_or_command",
            }
            .into());
        }
    }

    let init_json = serde_json::to_string(init_commands).map_err(|e| DbError::CommandsJsonEncode {
        column: "init_commands_json",
        source: e,
    })?;
    let run_json = serde_json::to_string(run_commands).map_err(|e| DbError::CommandsJsonEncode {
        column: "run_commands_json",
        source: e,
    })?;
    let now = chrono::Utc::now().timestamp();

    let tail = super::upsert::build_update_tail(
        adapter.backend(),
        &["user_id", "workspace_name"],
        &["init_commands_json", "run_commands_json", "updated_at"],
    );
    let sql = format!(
        "INSERT INTO user_worktree_commands \
            (user_id, workspace_name, init_commands_json, run_commands_json, updated_at) \
         VALUES (?, ?, ?, ?, ?) {tail}"
    );
    adapter
        .execute(
            &sql,
            vec![
                DbValue::Text(user_id.to_string()),
                DbValue::Text(workspace_name.to_string()),
                DbValue::Text(init_json),
                DbValue::Text(run_json),
                DbValue::I64(now),
            ],
        )
        .await?;
    Ok(())
}

/// Remove the row for `(user_id, workspace_name)`. Returns `true` if a row
/// was deleted, `false` if none existed.
pub async fn delete(
    adapter: &DbAdapter,
    user_id: &str,
    workspace_name: &str,
) -> Result<bool> {
    let affected = adapter
        .execute(
            "DELETE FROM user_worktree_commands WHERE user_id = ? AND workspace_name = ?",
            vec![
                DbValue::Text(user_id.to_string()),
                DbValue::Text(workspace_name.to_string()),
            ],
        )
        .await?;
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
pub async fn get_run_commands_for_pairs(
    adapter: &DbAdapter,
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

    let mut binds: Vec<DbValue> = Vec::with_capacity(pairs.len() * 2);
    for (uid, ws) in pairs {
        binds.push(DbValue::Text((*uid).to_string()));
        binds.push(DbValue::Text((*ws).to_string()));
    }

    let rows = adapter.query_all(&sql, binds).await?;

    let mut out: HashMap<(String, String), Vec<RunCommand>> = HashMap::new();
    for r in &rows {
        let uid = r.get_text(0)?;
        let ws = r.get_text(1)?;
        let run_json = r.get_text(2)?;
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

/// Decode the canonical 5-column row shape into a `UserWorktreeCommandsRow`.
/// JSON parse failures bubble as `DbError::CommandsJsonDecode` — caller
/// decides whether to propagate or omit.
fn decode_full_row(row: &crate::db::DbRow) -> Result<UserWorktreeCommandsRow> {
    let user_id = row.get_text(0)?;
    let workspace_name = row.get_text(1)?;
    let init_json = row.get_text(2)?;
    let run_json = row.get_text(3)?;
    let updated_at = row.get_i64(4)?;

    let init_commands = serde_json::from_str::<Vec<String>>(&init_json).map_err(|e| {
        DbError::CommandsJsonDecode {
            column: "init_commands_json",
            user_id: user_id.clone(),
            workspace_name: workspace_name.clone(),
            source: e,
        }
    })?;
    let run_commands = serde_json::from_str::<Vec<RunCommand>>(&run_json).map_err(|e| {
        DbError::CommandsJsonDecode {
            column: "run_commands_json",
            user_id: user_id.clone(),
            workspace_name: workspace_name.clone(),
            source: e,
        }
    })?;

    Ok(UserWorktreeCommandsRow {
        user_id,
        workspace_name,
        init_commands,
        run_commands,
        updated_at,
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
    /// set applied. Returns the adapter only — callers seed users as
    /// needed via `seed_user` so they control which test users exist.
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

    /// Seed a user row directly via the adapter (the `users` DAO is still
    /// on the legacy rusqlite path; this lets us bypass it in tests).
    /// Returns the user_id.
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
    async fn get_returns_none_on_missing() {
        let a = fresh_adapter().await;
        let alice = seed_user(&a, "alice", "user").await;
        assert!(get(&a, &alice, "frontend").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn upsert_then_get_roundtrips_both_kinds_in_order() {
        let a = fresh_adapter().await;
        let alice = seed_user(&a, "alice", "user").await;

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

        upsert(&a, &alice, "frontend", &init, &run).await.unwrap();

        let row = get(&a, &alice, "frontend").await.unwrap().unwrap();
        assert_eq!(row.user_id, alice);
        assert_eq!(row.workspace_name, "frontend");
        assert_eq!(row.init_commands, init, "init order must be preserved");
        assert_eq!(row.run_commands, run, "run order must be preserved");
        assert!(row.updated_at > 0);
    }

    #[tokio::test]
    async fn upsert_updates_updated_at_on_overwrite() {
        let a = fresh_adapter().await;
        let alice = seed_user(&a, "alice", "user").await;

        upsert(&a, &alice, "frontend", &["echo first".to_string()], &[])
            .await
            .unwrap();
        let first = get(&a, &alice, "frontend").await.unwrap().unwrap();

        // Force the recorded timestamp backward so the next upsert is
        // guaranteed strictly greater than the prior value, even on fast
        // machines where the timestamp would otherwise tick once per second.
        a.execute(
            "UPDATE user_worktree_commands SET updated_at = ? \
             WHERE user_id = ? AND workspace_name = 'frontend'",
            vec![
                DbValue::I64(first.updated_at - 100),
                DbValue::Text(alice.clone()),
            ],
        )
        .await
        .unwrap();

        upsert(
            &a,
            &alice,
            "frontend",
            &["echo second".to_string()],
            &[RunCommand {
                name: "Dev".to_string(),
                command: "npm run dev".to_string(),
            }],
        )
        .await
        .unwrap();
        let second = get(&a, &alice, "frontend").await.unwrap().unwrap();

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

    #[tokio::test]
    async fn delete_returns_true_then_false() {
        let a = fresh_adapter().await;
        let alice = seed_user(&a, "alice", "user").await;
        upsert(&a, &alice, "frontend", &["echo hi".to_string()], &[])
            .await
            .unwrap();

        assert!(delete(&a, &alice, "frontend").await.unwrap());
        assert!(!delete(&a, &alice, "frontend").await.unwrap());
        assert!(get(&a, &alice, "frontend").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn list_for_user_returns_only_their_rows() {
        let a = fresh_adapter().await;
        let alice = seed_user(&a, "alice", "user").await;
        let bob = seed_user(&a, "bob", "user").await;

        upsert(&a, &alice, "frontend", &["a".to_string()], &[])
            .await
            .unwrap();
        upsert(&a, &alice, "backend", &["b".to_string()], &[])
            .await
            .unwrap();
        upsert(&a, &bob, "frontend", &["b-frontend".to_string()], &[])
            .await
            .unwrap();

        let alice_rows = list_for_user(&a, &alice).await.unwrap();
        assert_eq!(alice_rows.len(), 2);
        for r in &alice_rows {
            assert_eq!(r.user_id, alice, "isolation: alice sees only her rows");
        }

        let bob_rows = list_for_user(&a, &bob).await.unwrap();
        assert_eq!(bob_rows.len(), 1);
        assert_eq!(bob_rows[0].workspace_name, "frontend");
        assert_eq!(bob_rows[0].init_commands, vec!["b-frontend".to_string()]);
    }

    #[tokio::test]
    async fn list_for_user_orders_by_updated_at_desc() {
        let a = fresh_adapter().await;
        let alice = seed_user(&a, "alice", "user").await;

        upsert(&a, &alice, "first", &["a".to_string()], &[])
            .await
            .unwrap();
        upsert(&a, &alice, "second", &["b".to_string()], &[])
            .await
            .unwrap();
        upsert(&a, &alice, "third", &["c".to_string()], &[])
            .await
            .unwrap();

        // Force a deterministic ordering: third > second > first.
        for (workspace, ts) in [("third", 300), ("second", 200), ("first", 100)] {
            a.execute(
                "UPDATE user_worktree_commands SET updated_at = ? \
                 WHERE user_id = ? AND workspace_name = ?",
                vec![
                    DbValue::I64(ts),
                    DbValue::Text(alice.clone()),
                    DbValue::Text(workspace.to_string()),
                ],
            )
            .await
            .unwrap();
        }

        let rows = list_for_user(&a, &alice).await.unwrap();
        let names: Vec<&str> = rows.iter().map(|r| r.workspace_name.as_str()).collect();
        assert_eq!(names, vec!["third", "second", "first"]);
    }

    #[tokio::test]
    async fn get_run_commands_for_pairs_batches_correctly() {
        let a = fresh_adapter().await;
        let alice = seed_user(&a, "alice", "user").await;
        let bob = seed_user(&a, "bob", "user").await;

        let run_a_frontend = vec![RunCommand {
            name: "Dashboard".to_string(),
            command: "npm run dev".to_string(),
        }];
        let run_b_frontend = vec![RunCommand {
            name: "Storybook".to_string(),
            command: "npm run sb".to_string(),
        }];

        upsert(&a, &alice, "frontend", &[], &run_a_frontend)
            .await
            .unwrap();
        upsert(&a, &alice, "backend", &[], &[]).await.unwrap();
        upsert(&a, &bob, "frontend", &[], &run_b_frontend)
            .await
            .unwrap();

        // Mix of hits and misses.
        let alice_ref: &str = &alice;
        let bob_ref: &str = &bob;
        let pairs: Vec<(&str, &str)> = vec![
            (alice_ref, "frontend"),       // hit, non-empty run
            (alice_ref, "backend"),        // hit, empty run
            (alice_ref, "does-not-exist"), // miss
            (bob_ref, "frontend"),         // hit, non-empty run
            (bob_ref, "backend"),          // miss (bob has no backend row)
        ];

        let map = get_run_commands_for_pairs(&a, &pairs).await.unwrap();
        assert_eq!(map.len(), 3, "three hits expected, got: {map:?}");

        assert_eq!(
            map.get(&(alice.clone(), "frontend".to_string())),
            Some(&run_a_frontend)
        );
        assert_eq!(
            map.get(&(alice.clone(), "backend".to_string())),
            Some(&vec![])
        );
        assert_eq!(
            map.get(&(bob.clone(), "frontend".to_string())),
            Some(&run_b_frontend)
        );
        // Misses are absent from the map (not present as empty vecs).
        assert!(
            !map.contains_key(&(alice.clone(), "does-not-exist".to_string())),
            "miss should be absent, not empty"
        );
        assert!(!map.contains_key(&(bob.clone(), "backend".to_string())));
    }

    #[tokio::test]
    async fn get_run_commands_for_pairs_empty_input_returns_empty_map() {
        let a = fresh_adapter().await;
        let map = get_run_commands_for_pairs(&a, &[]).await.unwrap();
        assert!(map.is_empty());
    }

    #[tokio::test]
    async fn nul_byte_in_command_is_rejected() {
        let a = fresh_adapter().await;
        let alice = seed_user(&a, "alice", "user").await;

        // NUL in an init command.
        let err = upsert(&a, &alice, "frontend", &["foo\0bar".to_string()], &[]).await;
        assert!(err.is_err(), "NUL in init command must be rejected");

        // NUL in a run-command name.
        let err = upsert(
            &a,
            &alice,
            "frontend",
            &[],
            &[RunCommand {
                name: "bad\0name".to_string(),
                command: "echo".to_string(),
            }],
        )
        .await;
        assert!(err.is_err(), "NUL in run-command name must be rejected");

        // NUL in a run-command command.
        let err = upsert(
            &a,
            &alice,
            "frontend",
            &[],
            &[RunCommand {
                name: "ok".to_string(),
                command: "echo \0".to_string(),
            }],
        )
        .await;
        assert!(err.is_err(), "NUL in run-command command must be rejected");

        // NUL in workspace_name.
        let err = upsert(&a, &alice, "fro\0nt", &[], &[]).await;
        assert!(err.is_err(), "NUL in workspace_name must be rejected");

        // No row should have been written by any of the failures above.
        assert!(get(&a, &alice, "frontend").await.unwrap().is_none());
    }

    /// Deleting a user cascades to their `user_worktree_commands` rows
    /// — the FK cascade is what we're really testing.
    #[tokio::test]
    async fn fk_cascade_deletes_rows_on_user_delete() {
        let a = fresh_adapter().await;
        let alice = seed_user(&a, "alice", "admin").await;
        // Second admin so the "last admin" guard wouldn't block a delete via
        // users::delete_user (not used here, but documents intent).
        let _bob = seed_user(&a, "bob", "admin").await;

        upsert(
            &a,
            &alice,
            "frontend",
            &["echo hi".to_string()],
            &[RunCommand {
                name: "Dev".to_string(),
                command: "npm run dev".to_string(),
            }],
        )
        .await
        .unwrap();
        upsert(&a, &alice, "backend", &["echo b".to_string()], &[])
            .await
            .unwrap();

        // Sanity: alice has 2 rows.
        assert_eq!(list_for_user(&a, &alice).await.unwrap().len(), 2);

        // Direct DELETE — bypasses users DAO (still rusqlite). The
        // ON DELETE CASCADE on user_worktree_commands.user_id is what we
        // exercise.
        a.execute(
            "DELETE FROM users WHERE id = ?",
            vec![DbValue::Text(alice.clone())],
        )
        .await
        .unwrap();

        // After cascade, alice has 0 rows.
        assert_eq!(list_for_user(&a, &alice).await.unwrap().len(), 0);
        // Direct count, too — guards against accidentally hiding via WHERE
        // user_id filter.
        let row = a
            .query_one("SELECT COUNT(*) FROM user_worktree_commands", vec![])
            .await
            .unwrap();
        let total = row.get_i64(0).unwrap();
        assert_eq!(total, 0, "FK cascade must drop every row alice owned");
    }

    /// JSON shape sanity check: init = list of strings, run = list of objects
    /// with `name` + `command`. Asserted by reading the raw JSON columns and
    /// re-parsing them with a strict typed schema.
    #[tokio::test]
    async fn json_shapes_round_trip_correctly() {
        let a = fresh_adapter().await;
        let alice = seed_user(&a, "alice", "user").await;

        upsert(
            &a,
            &alice,
            "frontend",
            &["echo a".to_string(), "echo b".to_string()],
            &[RunCommand {
                name: "Dev".to_string(),
                command: "npm run dev".to_string(),
            }],
        )
        .await
        .unwrap();

        let row = a
            .query_one(
                "SELECT init_commands_json, run_commands_json FROM user_worktree_commands \
                 WHERE user_id = ? AND workspace_name = 'frontend'",
                vec![DbValue::Text(alice)],
            )
            .await
            .unwrap();
        let init_raw = row.get_text(0).unwrap();
        let run_raw = row.get_text(1).unwrap();

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
    #[tokio::test]
    async fn fk_rejects_unknown_user_id() {
        let a = fresh_adapter().await;
        // No user created — direct upsert should fail at the FK.
        let res = upsert(&a, "no-such-user-id", "frontend", &[], &[]).await;
        assert!(res.is_err(), "FK should reject unknown user_id");
    }

    /// Empty init/run arrays round-trip as `[]` and parse back to empty Vecs.
    #[tokio::test]
    async fn upsert_accepts_both_empty_arrays() {
        let a = fresh_adapter().await;
        let alice = seed_user(&a, "alice", "user").await;
        upsert(&a, &alice, "frontend", &[], &[]).await.unwrap();

        let row = get(&a, &alice, "frontend").await.unwrap().unwrap();
        assert!(row.init_commands.is_empty());
        assert!(row.run_commands.is_empty());
    }
}
