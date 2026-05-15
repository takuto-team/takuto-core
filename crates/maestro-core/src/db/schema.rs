// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Database schema definitions and migration runner.

use crate::error::Result;

/// Current schema version. Increment when adding new migrations.
// v6 reserved for plan-03 audit log
const SCHEMA_VERSION: i32 = 5;

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

/// Plan-10 "per-user repositories" migration.
///
/// Adds the `repositories` registry — one row per on-disk clone under
/// `WORKSPACES_DIR` — and reshapes the (previously unused) `user_repositories`
/// table to FK to it.
///
/// Key shape decisions (from plan-10 review):
/// - `repositories.name` is **NOT UNIQUE**. Two forks (e.g. `owner-a/foo` and
///   `owner-b/foo`) collide on `name=foo` but must coexist; the clone-time
///   path collision resolver suffixes `-2`, `-3`, … on `local_path`. UUID `id`
///   is the durable identity; `local_path` is the on-disk uniqueness key.
/// - `repo_url` is NOT UNIQUE — re-registering the same URL at a different
///   path is valid; uniqueness lives on `local_path`.
/// - `created_by` → `users(id) ON DELETE SET NULL` so deleting the user who
///   originally cloned a repo keeps the registration intact (it's a shared
///   on-disk artefact).
/// - `user_repositories` is dropped and recreated with composite PK
///   `(user_id, repository_id)`, both FKs cascading. There's no migration of
///   the old shape's data because plan-01 reserved it but no code ever wrote
///   to it (every row in the wild is necessarily empty).
const MIGRATION_V5: &str = r#"
CREATE TABLE repositories (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    repo_url TEXT,
    local_path TEXT NOT NULL UNIQUE,
    default_branch TEXT NOT NULL DEFAULT 'main',
    created_at INTEGER NOT NULL,
    created_by TEXT,
    FOREIGN KEY (created_by) REFERENCES users(id) ON DELETE SET NULL
);
CREATE INDEX idx_repositories_name ON repositories(name);
CREATE INDEX idx_repositories_repo_url ON repositories(repo_url);

DROP TABLE IF EXISTS user_repositories;

CREATE TABLE user_repositories (
    user_id TEXT NOT NULL,
    repository_id TEXT NOT NULL,
    added_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, repository_id),
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE,
    FOREIGN KEY (repository_id) REFERENCES repositories(id) ON DELETE CASCADE
);
CREATE INDEX idx_user_repositories_repo ON user_repositories(repository_id);
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

    if current_version < 5 {
        conn.execute_batch(MIGRATION_V5)?;
        conn.execute(
            "INSERT INTO schema_migrations (version) VALUES (?1)",
            rusqlite::params![5],
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

    /// Plan-09 AC: a fresh database applies migrations through v4's table
    /// changes — the new `user_worktree_commands` table exists with the
    /// expected columns; plan-08's `workspace_commands` must NOT exist (it's
    /// dropped by v4 even on a fresh install — the `DROP TABLE IF EXISTS` is
    /// a no-op since v3's `CREATE` never ran in isolation, but the assertion
    /// guards against accidental reintroduction).
    ///
    /// Plan-10 note: `SCHEMA_VERSION` is now 5, so we no longer assert the
    /// version is exactly 4 — we assert it's **at least** 4 and that all v4
    /// table-shape invariants still hold. `fresh_db_applies_v5` covers the
    /// post-v5 invariants.
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
        assert!(
            version >= 4,
            "v4 must be applied on a fresh DB; got version {version}"
        );

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

        // Run the migration runner: it should apply v4 (and any later
        // migrations that have since landed — plan-10 added v5).
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

        // Version is at least 4 (will be SCHEMA_VERSION after any later
        // migrations chain on top — we only assert the v3→v4 invariants here).
        let version: i32 = conn
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(
            version >= 4,
            "expected migration runner to reach at least v4; got {version}"
        );
        assert_eq!(version, SCHEMA_VERSION);
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
            rows as i32, SCHEMA_VERSION,
            "expected exactly SCHEMA_VERSION ({SCHEMA_VERSION}) migration rows; got {rows}"
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

    /// Plan-10 AC: a fresh database applies migrations through v5 and the
    /// new `repositories` table exists with the expected schema. The
    /// `user_repositories` table is reshaped (composite PK
    /// `(user_id, repository_id)`; the legacy v1 columns `repo_url` /
    /// `local_path` / `id` are gone).
    #[test]
    fn fresh_db_applies_v5() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        run_migrations(&conn).unwrap();

        let version: i32 = conn
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(version, 5);

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
            tables.contains(&"repositories".to_string()),
            "repositories table missing; got: {tables:?}"
        );
        assert!(
            tables.contains(&"user_repositories".to_string()),
            "user_repositories table missing; got: {tables:?}"
        );

        // repositories columns.
        let repo_cols: Vec<String> = {
            let mut stmt = conn.prepare("PRAGMA table_info(repositories)").unwrap();
            stmt.query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .filter_map(|r| r.ok())
                .collect()
        };
        for expected in [
            "id",
            "name",
            "repo_url",
            "local_path",
            "default_branch",
            "created_at",
            "created_by",
        ] {
            assert!(
                repo_cols.iter().any(|c| c == expected),
                "missing repositories column {expected}; got: {repo_cols:?}"
            );
        }

        // user_repositories columns are the reshaped composite PK.
        let ur_cols: Vec<String> = {
            let mut stmt = conn
                .prepare("PRAGMA table_info(user_repositories)")
                .unwrap();
            stmt.query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .filter_map(|r| r.ok())
                .collect()
        };
        for expected in ["user_id", "repository_id", "added_at"] {
            assert!(
                ur_cols.iter().any(|c| c == expected),
                "missing user_repositories column {expected}; got: {ur_cols:?}"
            );
        }
        // Legacy v1 columns are gone.
        for legacy in ["repo_url", "local_path"] {
            assert!(
                !ur_cols.iter().any(|c| c == legacy),
                "legacy user_repositories column {legacy} should have been dropped; got: {ur_cols:?}"
            );
        }

        // Indexes exist.
        let indexes: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT name FROM sqlite_master WHERE type='index' AND tbl_name IN ('repositories', 'user_repositories')")
                .unwrap();
            stmt.query_map([], |row| row.get(0))
                .unwrap()
                .filter_map(|r| r.ok())
                .collect()
        };
        for expected in [
            "idx_repositories_name",
            "idx_repositories_repo_url",
            "idx_user_repositories_repo",
        ] {
            assert!(
                indexes.iter().any(|i| i == expected),
                "missing index {expected}; got: {indexes:?}"
            );
        }
    }

    /// Plan-10: upgrading a v4-only database to v5 drops the old
    /// `user_repositories` shape (plan-01 reserved but never wrote to it) and
    /// creates the new `repositories` table + reshaped `user_repositories`.
    /// Unrelated tables (users, sessions, etc.) are preserved.
    #[test]
    fn v4_to_v5_upgrade_creates_repositories_and_reshapes_user_repositories() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();

        // Apply v1–v4 by hand, then stamp the migration log so the runner
        // thinks it's resuming from a v4 install.
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
        conn.execute_batch(MIGRATION_V4).unwrap();
        for v in 1..=4 {
            conn.execute(
                "INSERT INTO schema_migrations (version) VALUES (?1)",
                rusqlite::params![v],
            )
            .unwrap();
        }

        // Pre-existing row in unrelated table.
        conn.execute(
            "INSERT INTO users (id, username, role) VALUES ('u-pre', 'preexisting', 'admin')",
            [],
        )
        .unwrap();

        // Run the migration runner: it should apply only v5.
        run_migrations(&conn).unwrap();

        // Pre-existing users row still present.
        let users: i64 = conn
            .query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))
            .unwrap();
        assert_eq!(users, 1);

        // New repositories table is empty and present.
        let new_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM repositories", [], |r| r.get(0))
            .unwrap();
        assert_eq!(new_count, 0);

        // Reshaped user_repositories has the new composite PK columns.
        let ur_cols: Vec<String> = {
            let mut stmt = conn
                .prepare("PRAGMA table_info(user_repositories)")
                .unwrap();
            stmt.query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .filter_map(|r| r.ok())
                .collect()
        };
        assert!(ur_cols.iter().any(|c| c == "repository_id"));
        assert!(!ur_cols.iter().any(|c| c == "repo_url"));

        // Version bumped to 5.
        let version: i32 = conn
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(version, 5);
    }

    /// Plan-10: running `run_migrations` twice against a v5 DB is a no-op.
    #[test]
    fn v5_migrations_are_idempotent() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();

        run_migrations(&conn).unwrap();
        run_migrations(&conn).unwrap();

        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            rows, 5,
            "expected exactly 5 migration rows (v1..v5); got {rows}"
        );

        let new_table_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='repositories'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(new_table_exists, 1);
    }

    /// Plan-10 G12: the comment at the top of `schema.rs` reserves the next
    /// migration slot (v6) for plan-03's audit log. This test guards the
    /// reservation by verifying that `SCHEMA_VERSION == 5` (so anyone bumping
    /// to v6 has to also touch the reservation note).
    #[test]
    fn v6_is_reserved_for_plan_03_audit_log() {
        assert_eq!(SCHEMA_VERSION, 5, "if you bump SCHEMA_VERSION, move the 'reserved for plan-03 audit log' comment in schema.rs from v6 to the next slot");
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
