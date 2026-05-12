// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Database schema definitions and migration runner.

use crate::error::Result;

/// Current schema version. Increment when adding new migrations.
const SCHEMA_VERSION: i32 = 1;

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

    // Future migrations go here:
    // if current_version < 2 { ... }

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
    }
}
