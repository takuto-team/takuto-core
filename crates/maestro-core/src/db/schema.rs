// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Database schema definitions and migration runner.

use crate::error::Result;

/// Current schema version. Increment when adding new migrations.
// v5 reserved for plan-03 audit log
const SCHEMA_VERSION: i32 = 4;

const MIGRATION_V1: &str = r#"
CREATE TABLE IF NOT EXISTS users (
    id TEXT PRIMARY KEY NOT NULL,
    username TEXT UNIQUE NOT NULL,
    role TEXT NOT NULL DEFAULT 'user' CHECK(role IN ('admin', 'user')),
    suspended INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE TABLE IF NOT EXISTS credentials (
    id TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK(kind IN ('password', 'passkey')),
    data BLOB NOT NULL,
    label TEXT,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    last_used_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_credentials_user_id ON credentials(user_id);

CREATE TABLE IF NOT EXISTS recovery_codes (
    id TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    code_hash BLOB NOT NULL,
    used INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);
CREATE INDEX IF NOT EXISTS idx_recovery_codes_user_id ON recovery_codes(user_id);

CREATE TABLE IF NOT EXISTS user_repositories (
    id TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    repo_url TEXT NOT NULL,
    local_path TEXT NOT NULL,
    added_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    UNIQUE(user_id, repo_url)
);
CREATE INDEX IF NOT EXISTS idx_user_repositories_user_id ON user_repositories(user_id);

CREATE TABLE IF NOT EXISTS container_users (
    id TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    container_id TEXT NOT NULL,
    container_type TEXT NOT NULL CHECK(container_type IN ('workflow', 'terminal', 'editor')),
    os_username TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    destroyed_at TEXT,
    UNIQUE(user_id, container_id)
);
CREATE INDEX IF NOT EXISTS idx_container_users_user_id ON container_users(user_id);

CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    data BLOB NOT NULL,
    expires_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_sessions_user_id ON sessions(user_id);
CREATE INDEX IF NOT EXISTS idx_sessions_expires_at ON sessions(expires_at);
"#;

/// Plan-02 "auth hardening" migration.
///
/// Adds the persistent per-user `login_attempts` audit table (AC-3 — rate-limit
/// + lockout) and two new columns on `sessions` (AC-5 — sliding-extend +
/// absolute-TTL session rotation):
///
/// - `sessions.last_seen_at INTEGER` — unix seconds; bumped at most every
///   `SESSION_EXTEND_THRESHOLD_SECS` from the auth middleware so an active
///   session's idle clock slides forward.
/// - `sessions.created_at_unix INTEGER` — unix seconds at insertion; used to
///   enforce the **absolute** 30-day TTL even for actively-used sessions.
///
/// Backfill semantics: both new columns default to `0`. Sessions inserted
/// under v1 will have `created_at_unix = 0`, which is older than `now - 30d`
/// for any realistic clock, so the absolute-TTL check will reject and
/// lazily delete them on next use — that's the intended "force re-login
/// after the upgrade for any session older than the rollout" behaviour.
const MIGRATION_V2: &str = r#"
CREATE TABLE IF NOT EXISTS login_attempts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK(kind IN ('password','recovery')),
    attempted_at INTEGER NOT NULL,
    success INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_login_attempts_user_kind_time
    ON login_attempts(user_id, kind, attempted_at);

ALTER TABLE sessions ADD COLUMN last_seen_at INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN created_at_unix INTEGER NOT NULL DEFAULT 0;
CREATE INDEX IF NOT EXISTS idx_sessions_last_seen_at ON sessions(last_seen_at);
"#;

/// Plan-08 "worktree_init_commands per-workspace overrides" migration.
///
/// Adds the `workspace_commands` table, which stores admin-authored per-workspace
/// overrides for `[commands].worktree_init_commands`. The engine reads this table
/// at workflow bootstrap to pick between the workspace override and the global
/// `Config.commands.worktree_init_commands` default.
///
/// `commands_json` is the serialized JSON array of strings (sort order matters,
/// each entry is a single `bash -lc` invocation). `updated_by` references the
/// admin's user_id with `ON DELETE SET NULL` — deleting the admin keeps the
/// override row (operators still need their commands) but drops the audit link.
const MIGRATION_V3: &str = r#"
CREATE TABLE IF NOT EXISTS workspace_commands (
    workspace_name TEXT PRIMARY KEY,
    commands_json TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    updated_by TEXT,
    FOREIGN KEY (updated_by) REFERENCES users(id) ON DELETE SET NULL
);
CREATE INDEX IF NOT EXISTS idx_workspace_commands_updated ON workspace_commands(updated_at DESC);
"#;

/// Plan-09 "per-user worktree settings" migration.
///
/// Drops plan-08's admin-scoped `workspace_commands` table (plan-08 was never
/// released, so no production data to migrate) and replaces it with
/// `user_worktree_commands`, keyed by `(user_id, workspace_name)`. Each row
/// stores BOTH command kinds — `init_commands_json` (a JSON array of strings
/// run at worktree bootstrap) and `run_commands_json` (a JSON array of
/// `{name, command}` objects surfaced as buttons on completed workflow cards).
///
/// Two JSON columns rather than two tables: a single round-trip per
/// `(user, workspace)` lookup, atomic updates, and fewer endpoints. The
/// application layer knows the schema for each column.
///
/// `user_id` cascades on user delete — removing a user wipes every row they
/// configured (AC-7).
const MIGRATION_V4: &str = r#"
DROP TABLE IF EXISTS workspace_commands;

CREATE TABLE user_worktree_commands (
    user_id TEXT NOT NULL,
    workspace_name TEXT NOT NULL,
    init_commands_json TEXT NOT NULL DEFAULT '[]',
    run_commands_json TEXT NOT NULL DEFAULT '[]',
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, workspace_name),
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);
CREATE INDEX idx_user_worktree_commands_user ON user_worktree_commands(user_id, updated_at DESC);
"#;

/// Run all pending migrations. Idempotent — safe to call on every startup.
pub fn run_migrations(conn: &rusqlite::Connection) -> Result<()> {
    // Create the migration tracking table if it doesn't exist.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY NOT NULL,
            applied_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
        );",
    )?;

    let current_version: i32 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
        [],
        |row| row.get(0),
    )?;

    if current_version < 1 {
        conn.execute_batch(MIGRATION_V1)?;
        conn.execute(
            "INSERT INTO schema_migrations (version) VALUES (?1)",
            rusqlite::params![1],
        )?;
    }

    if current_version < 2 {
        conn.execute_batch(MIGRATION_V2)?;
        conn.execute(
            "INSERT INTO schema_migrations (version) VALUES (?1)",
            rusqlite::params![2],
        )?;
    }

    if current_version < 3 {
        conn.execute_batch(MIGRATION_V3)?;
        conn.execute(
            "INSERT INTO schema_migrations (version) VALUES (?1)",
            rusqlite::params![3],
        )?;
    }

    if current_version < 4 {
        conn.execute_batch(MIGRATION_V4)?;
        conn.execute(
            "INSERT INTO schema_migrations (version) VALUES (?1)",
            rusqlite::params![4],
        )?;
    }

    let final_version: i32 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
        [],
        |row| row.get(0),
    )?;

    if final_version != SCHEMA_VERSION {
        return Err(crate::error::MaestroError::Database(format!(
            "Schema migration failed: expected version {SCHEMA_VERSION}, got {final_version}"
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_are_idempotent() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();

        // Run migrations twice — should not fail.
        run_migrations(&conn).unwrap();
        run_migrations(&conn).unwrap();

        // Verify tables exist by querying them.
        let count: i32 = conn
            .query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn schema_version_is_tracked() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        run_migrations(&conn).unwrap();

        let version: i32 = conn
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn all_tables_created() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        run_migrations(&conn).unwrap();

        let tables: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
                .unwrap();
            stmt.query_map([], |row| row.get(0))
                .unwrap()
                .filter_map(|r| r.ok())
                .collect()
        };

        assert!(tables.contains(&"users".to_string()));
        assert!(tables.contains(&"credentials".to_string()));
        assert!(tables.contains(&"recovery_codes".to_string()));
        assert!(tables.contains(&"user_repositories".to_string()));
        assert!(tables.contains(&"container_users".to_string()));
        assert!(tables.contains(&"sessions".to_string()));
        assert!(tables.contains(&"schema_migrations".to_string()));
        // Plan-09: plan-08's `workspace_commands` is replaced by
        // `user_worktree_commands` in v4. The old name must NOT be present.
        assert!(!tables.contains(&"workspace_commands".to_string()));
        assert!(tables.contains(&"user_worktree_commands".to_string()));
    }

    /// Plan-02 AC-3 + AC-5: the v2 migration creates `login_attempts` and
    /// adds `last_seen_at` and `created_at_unix` columns to `sessions`.
    #[test]
    fn all_tables_created_v2() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        run_migrations(&conn).unwrap();

        // login_attempts table exists.
        let tables: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
                .unwrap();
            stmt.query_map([], |row| row.get(0))
                .unwrap()
                .filter_map(|r| r.ok())
                .collect()
        };
        assert!(tables.contains(&"login_attempts".to_string()));

        // sessions.last_seen_at and sessions.created_at_unix columns exist.
        let cols: Vec<String> = {
            let mut stmt = conn.prepare("PRAGMA table_info(sessions)").unwrap();
            stmt.query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .filter_map(|r| r.ok())
                .collect()
        };
        assert!(
            cols.iter().any(|c| c == "last_seen_at"),
            "sessions.last_seen_at column missing; got: {cols:?}"
        );
        assert!(
            cols.iter().any(|c| c == "created_at_unix"),
            "sessions.created_at_unix column missing; got: {cols:?}"
        );
    }

    /// Plan-02 AC-3: `login_attempts.kind` is constrained to ('password','recovery');
    /// foreign key cascades on user delete.
    #[test]
    fn login_attempts_check_constraint_and_cascade() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        run_migrations(&conn).unwrap();

        // Insert a user.
        conn.execute(
            "INSERT INTO users (id, username, role) VALUES ('u1', 'alice', 'admin')",
            [],
        )
        .unwrap();

        // Valid kind: ok.
        conn.execute(
            "INSERT INTO login_attempts (user_id, kind, attempted_at, success) VALUES ('u1', 'password', 100, 0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO login_attempts (user_id, kind, attempted_at, success) VALUES ('u1', 'recovery', 200, 0)",
            [],
        )
        .unwrap();

        // Invalid kind: rejected by CHECK constraint.
        let bad = conn.execute(
            "INSERT INTO login_attempts (user_id, kind, attempted_at, success) VALUES ('u1', 'webauthn', 300, 0)",
            [],
        );
        assert!(bad.is_err(), "CHECK constraint should reject unknown kinds");

        // Cascade on user delete.
        conn.execute("DELETE FROM users WHERE id='u1'", []).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM login_attempts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "FK cascade should drop login_attempts rows");
    }

    /// Plan-09 AC: a fresh database applies migrations through v4 and the
    /// new `user_worktree_commands` table exists with the expected columns.
    /// The plan-08 `workspace_commands` table must NOT exist (it's dropped
    /// by v4 even on a fresh install — the DROP TABLE IF EXISTS is a no-op
    /// since v3's CREATE never ran in isolation, but the assertion guards
    /// against accidental reintroduction).
    #[test]
    fn fresh_db_applies_v4() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        run_migrations(&conn).unwrap();

        let version: i32 = conn
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(version, 4);

        let tables: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
                .unwrap();
            stmt.query_map([], |row| row.get(0))
                .unwrap()
                .filter_map(|r| r.ok())
                .collect()
        };
        assert!(
            !tables.contains(&"workspace_commands".to_string()),
            "plan-08's workspace_commands must be dropped by v4; got: {tables:?}"
        );
        assert!(
            tables.contains(&"user_worktree_commands".to_string()),
            "user_worktree_commands table should exist after v4; got: {tables:?}"
        );

        // Expected columns.
        let cols: Vec<String> = {
            let mut stmt = conn
                .prepare("PRAGMA table_info(user_worktree_commands)")
                .unwrap();
            stmt.query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .filter_map(|r| r.ok())
                .collect()
        };
        for expected in [
            "user_id",
            "workspace_name",
            "init_commands_json",
            "run_commands_json",
            "updated_at",
        ] {
            assert!(
                cols.iter().any(|c| c == expected),
                "missing column {expected}; got: {cols:?}"
            );
        }

        // The composite (user, updated_at) index is present.
        let indexes: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT name FROM sqlite_master WHERE type='index' AND tbl_name='user_worktree_commands'")
                .unwrap();
            stmt.query_map([], |row| row.get(0))
                .unwrap()
                .filter_map(|r| r.ok())
                .collect()
        };
        assert!(
            indexes
                .iter()
                .any(|i| i == "idx_user_worktree_commands_user"),
            "missing idx_user_worktree_commands_user; got: {indexes:?}"
        );
    }

    /// Plan-09: upgrading a v3-only database to v4 drops the old
    /// `workspace_commands` table (including any rows in it — plan-08 was
    /// never released, so dropping data is the intentional behaviour) and
    /// creates the new `user_worktree_commands` table. Other tables
    /// (users, sessions, login_attempts) are preserved.
    #[test]
    fn v3_to_v4_upgrade_drops_workspace_commands_and_creates_new_table() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();

        // Apply v1 + v2 + v3 by hand, then mark as version 3 so the runner
        // thinks it's resuming from a v3 install.
        conn.execute_batch(
            "CREATE TABLE schema_migrations (
                version INTEGER PRIMARY KEY NOT NULL,
                applied_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
            );",
        )
        .unwrap();
        conn.execute_batch(MIGRATION_V1).unwrap();
        conn.execute_batch(MIGRATION_V2).unwrap();
        conn.execute_batch(MIGRATION_V3).unwrap();
        for v in 1..=3 {
            conn.execute(
                "INSERT INTO schema_migrations (version) VALUES (?1)",
                rusqlite::params![v],
            )
            .unwrap();
        }

        // Pre-existing rows that must survive.
        conn.execute(
            "INSERT INTO users (id, username, role) VALUES ('u-pre', 'preexisting', 'admin')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, user_id, data, expires_at) VALUES ('s1', 'u-pre', X'00', '2099-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO login_attempts (user_id, kind, attempted_at, success) VALUES ('u-pre', 'password', 100, 0)",
            [],
        )
        .unwrap();

        // Pre-existing row in the soon-to-be-dropped workspace_commands.
        conn.execute(
            "INSERT INTO workspace_commands (workspace_name, commands_json, updated_at, updated_by) \
             VALUES ('frontend', '[\"echo legacy\"]', 100, NULL)",
            [],
        )
        .unwrap();

        // Run the migration runner: it should apply only v4.
        run_migrations(&conn).unwrap();

        // Pre-existing rows in unrelated tables still present.
        let users: i64 = conn
            .query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))
            .unwrap();
        assert_eq!(users, 1);
        let sessions: i64 = conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(sessions, 1);
        let attempts: i64 = conn
            .query_row("SELECT COUNT(*) FROM login_attempts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(attempts, 1);

        // workspace_commands has been dropped.
        let tables: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")
                .unwrap();
            stmt.query_map([], |row| row.get(0))
                .unwrap()
                .filter_map(|r| r.ok())
                .collect()
        };
        assert!(
            !tables.contains(&"workspace_commands".to_string()),
            "workspace_commands must be dropped after v3→v4; got: {tables:?}"
        );

        // user_worktree_commands exists and is empty.
        let new_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM user_worktree_commands", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(new_count, 0);

        // Version bumped to 4.
        let version: i32 = conn
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(version, 4);
    }

    /// Plan-09: running `run_migrations` twice against a v4 DB is a no-op
    /// (no duplicate schema_migrations rows, no table conflicts).
    #[test]
    fn v4_migrations_are_idempotent() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();

        run_migrations(&conn).unwrap();
        run_migrations(&conn).unwrap();

        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            rows, 4,
            "expected exactly 4 migration rows (v1, v2, v3, v4); got {rows}"
        );

        // The new table still exists after a second migrate.
        let new_table_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='user_worktree_commands'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(new_table_exists, 1);
    }

    /// Plan-02 carried over: a DB whose schema version is newer than the
    /// binary's `SCHEMA_VERSION` triggers an error rather than silently
    /// running on an unknown schema.
    #[test]
    fn schema_newer_than_binary_is_rejected() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        run_migrations(&conn).unwrap();

        // Simulate a future migration already applied by a newer binary.
        conn.execute(
            "INSERT INTO schema_migrations (version) VALUES (?1)",
            rusqlite::params![SCHEMA_VERSION + 1],
        )
        .unwrap();

        let err = run_migrations(&conn);
        assert!(
            err.is_err(),
            "expected migration runner to reject a DB at version {} when binary expects {}",
            SCHEMA_VERSION + 1,
            SCHEMA_VERSION,
        );
    }

    /// Plan-02: upgrading a v1-only database to v2 keeps existing rows and
    /// reports the new schema version.
    #[test]
    fn v1_to_v2_upgrade_preserves_rows() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();

        // Apply v1 only by hand, then manually mark it as version 1 so the
        // runner thinks it's resuming from a v1 install.
        conn.execute_batch(
            "CREATE TABLE schema_migrations (
                version INTEGER PRIMARY KEY NOT NULL,
                applied_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
            );",
        )
        .unwrap();
        conn.execute_batch(MIGRATION_V1).unwrap();
        conn.execute(
            "INSERT INTO schema_migrations (version) VALUES (1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO users (id, username, role) VALUES ('u-pre', 'preexisting', 'admin')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, user_id, data, expires_at) VALUES ('s1', 'u-pre', X'00', '2099-01-01T00:00:00Z')",
            [],
        )
        .unwrap();

        // Now run the migration runner: it should apply v2 and v3.
        run_migrations(&conn).unwrap();

        // Pre-existing rows still present.
        let users: i64 = conn
            .query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))
            .unwrap();
        assert_eq!(users, 1);
        let sessions: i64 = conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(sessions, 1);

        // v2 column was backfilled with default 0.
        let last_seen: i64 = conn
            .query_row(
                "SELECT last_seen_at FROM sessions WHERE id='s1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(last_seen, 0);

        // Version bumped to the current SCHEMA_VERSION (v3).
        let version: i32 = conn
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }
}
