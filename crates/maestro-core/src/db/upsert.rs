// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Dialect-aware INSERT … upsert tail.
//!
//! `ON CONFLICT(col) DO UPDATE` is valid SQLite and Postgres syntax but
//! MySQL/MariaDB doesn't accept it. MySQL spells the same operation as
//! `ON DUPLICATE KEY UPDATE`, and `excluded.<col>` becomes `VALUES(<col>)`.
//! Rather than scatter `match adapter.backend()` blocks across every
//! upsert call site, this module exposes two small builders that emit
//! the right tail per backend. Call sites supply the conflict columns
//! and the set-list; the rest is plumbing.
//!
//! The helpers return owned `String`s because the SQL is built once per
//! call (every upsert site is on a cold path: write-heavy auth flows,
//! credential rotation, etc.). The allocation is in the noise compared
//! to the network round-trip that follows.

use super::pool::DbBackend;

/// Build the upsert tail for an `INSERT … VALUES (…)` where a row whose
/// `conflict_cols` collide with an existing one should overwrite the
/// listed `update_cols` with the new values.
///
/// SQLite / Postgres:
///   `ON CONFLICT(c1, c2) DO UPDATE SET col_a = excluded.col_a, col_b = excluded.col_b`
///
/// MySQL / MariaDB:
///   `ON DUPLICATE KEY UPDATE col_a = VALUES(col_a), col_b = VALUES(col_b)`
///
/// The MySQL form does not name the conflict columns — the engine
/// figures it out from the unique constraint that fired. Callers that
/// have multiple unique constraints on the same table should still
/// supply the right `conflict_cols` so the SQLite/Postgres branch
/// targets the intended one.
///
/// Both branches preserve the order of `update_cols`.
pub fn build_update_tail(
    backend: DbBackend,
    conflict_cols: &[&str],
    update_cols: &[&str],
) -> String {
    match backend {
        DbBackend::MySql => {
            let set = update_cols
                .iter()
                .map(|c| format!("{c} = VALUES({c})"))
                .collect::<Vec<_>>()
                .join(", ");
            format!("ON DUPLICATE KEY UPDATE {set}")
        }
        DbBackend::Sqlite | DbBackend::Postgres => {
            let set = update_cols
                .iter()
                .map(|c| format!("{c} = excluded.{c}"))
                .collect::<Vec<_>>()
                .join(", ");
            let conflict = conflict_cols.join(", ");
            format!("ON CONFLICT({conflict}) DO UPDATE SET {set}")
        }
    }
}

/// Build the upsert tail for an `INSERT … VALUES (…)` where a row whose
/// `conflict_cols` collide with an existing one should be **dropped**
/// silently (no update).
///
/// SQLite / Postgres: `ON CONFLICT(c1, c2) DO NOTHING`.
///
/// MySQL / MariaDB doesn't have a direct equivalent — `INSERT IGNORE`
/// drops on *any* constraint violation, which is too permissive. Use
/// a no-op update on the first conflict column instead: the unique
/// constraint still fires, but the SET clause assigns a column to its
/// own value, leaving the row unchanged.
pub fn build_ignore_tail(backend: DbBackend, conflict_cols: &[&str]) -> String {
    match backend {
        DbBackend::MySql => {
            // The no-op assignment must reference some column; using the
            // first conflict column is uniformly safe.
            // SAFETY: every caller passes a non-empty `conflict_cols` — an
            // upsert has no meaning without conflict columns — so an empty
            // slice is a programming error, not a runtime condition.
            let first = conflict_cols
                .first()
                .copied()
                .expect("conflict_cols must be non-empty");
            format!("ON DUPLICATE KEY UPDATE {first} = {first}")
        }
        DbBackend::Sqlite | DbBackend::Postgres => {
            let conflict = conflict_cols.join(", ");
            format!("ON CONFLICT({conflict}) DO NOTHING")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_tail_sqlite_uses_excluded_form() {
        let got = build_update_tail(
            DbBackend::Sqlite,
            &["user_id"],
            &["value", "updated_at"],
        );
        assert_eq!(
            got,
            "ON CONFLICT(user_id) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at"
        );
    }

    #[test]
    fn update_tail_postgres_matches_sqlite() {
        // Postgres and SQLite share the upsert syntax; the helper
        // returns identical strings for both.
        let sqlite = build_update_tail(DbBackend::Sqlite, &["k"], &["v"]);
        let postgres = build_update_tail(DbBackend::Postgres, &["k"], &["v"]);
        assert_eq!(sqlite, postgres);
    }

    #[test]
    fn update_tail_mysql_uses_values_form() {
        let got = build_update_tail(
            DbBackend::MySql,
            &["user_id"],
            &["value", "updated_at"],
        );
        assert_eq!(
            got,
            "ON DUPLICATE KEY UPDATE value = VALUES(value), updated_at = VALUES(updated_at)"
        );
    }

    #[test]
    fn update_tail_handles_composite_conflict_key() {
        let got = build_update_tail(
            DbBackend::Postgres,
            &["user_id", "workspace_name"],
            &["init_commands_json"],
        );
        assert_eq!(
            got,
            "ON CONFLICT(user_id, workspace_name) DO UPDATE SET init_commands_json = excluded.init_commands_json"
        );
    }

    #[test]
    fn update_tail_preserves_column_order() {
        let got = build_update_tail(DbBackend::MySql, &["k"], &["c", "a", "b"]);
        assert!(got.contains("c = VALUES(c), a = VALUES(a), b = VALUES(b)"));
    }

    #[test]
    fn ignore_tail_sqlite_uses_do_nothing() {
        let got = build_ignore_tail(DbBackend::Sqlite, &["local_path"]);
        assert_eq!(got, "ON CONFLICT(local_path) DO NOTHING");
    }

    #[test]
    fn ignore_tail_mysql_uses_noop_self_assign() {
        let got = build_ignore_tail(DbBackend::MySql, &["local_path"]);
        assert_eq!(got, "ON DUPLICATE KEY UPDATE local_path = local_path");
    }

    #[test]
    fn ignore_tail_composite_conflict_falls_back_to_first_col_on_mysql() {
        let got = build_ignore_tail(DbBackend::MySql, &["user_id", "repository_id"]);
        // Only the first column appears in the no-op self-assign — the
        // engine still uses the full constraint to detect the conflict.
        assert_eq!(got, "ON DUPLICATE KEY UPDATE user_id = user_id");
    }
}
