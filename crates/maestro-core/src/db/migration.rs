// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Legacy migration: config.toml single-user password -> SQLite multi-user.

use crate::config::WebConfig;
use crate::error::Result;

use super::models::UserRole;
use super::{credentials, users};

/// Migrate the legacy single-user config.toml credentials to the SQLite database.
///
/// If the `users` table is empty and `WebConfig` has non-empty `dashboard_username` +
/// `dashboard_password`, creates an admin user with the password hashed via argon2.
///
/// Returns `true` if migration occurred, `false` if skipped (already migrated or no
/// legacy creds).
pub fn migrate_legacy_credentials(
    conn: &rusqlite::Connection,
    web_config: &WebConfig,
) -> Result<bool> {
    let count = users::count_users(conn)?;
    if count > 0 {
        // Already migrated or users exist -- skip.
        return Ok(false);
    }

    if !web_config.dashboard_auth_enabled() {
        // No legacy credentials to migrate.
        return Ok(false);
    }

    let username = web_config.dashboard_username.trim();
    let password = &web_config.dashboard_password;

    // Create admin user (first user gets admin automatically).
    let user = users::create_user(conn, username, UserRole::Admin)?;

    // Store the hashed password.
    credentials::store_password(conn, &user.id, password)?;

    tracing::info!(
        username = username,
        user_id = %user.id,
        "Migrated legacy config.toml credentials to SQLite -- user created as admin"
    );

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WebConfig;
    use crate::db::schema;

    fn test_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        schema::run_migrations(&conn).unwrap();
        conn
    }

    #[test]
    fn migrates_when_legacy_creds_present() {
        let conn = test_conn();
        let web = WebConfig {
            dashboard_username: "admin".into(),
            dashboard_password: "secret123".into(),
            ..Default::default()
        };
        let migrated = migrate_legacy_credentials(&conn, &web).unwrap();
        assert!(migrated);

        // Verify user was created.
        let user = users::get_user_by_username(&conn, "admin")
            .unwrap()
            .unwrap();
        assert_eq!(user.role, UserRole::Admin);

        // Verify password works.
        assert!(credentials::verify_user_password(&conn, &user.id, "secret123").unwrap());
    }

    #[test]
    fn skips_when_no_legacy_creds() {
        let conn = test_conn();
        let web = WebConfig::default(); // empty username/password
        let migrated = migrate_legacy_credentials(&conn, &web).unwrap();
        assert!(!migrated);
        assert_eq!(users::count_users(&conn).unwrap(), 0);
    }

    #[test]
    fn skips_when_already_migrated() {
        let conn = test_conn();
        let web = WebConfig {
            dashboard_username: "admin".into(),
            dashboard_password: "secret123".into(),
            ..Default::default()
        };
        // First migration.
        let migrated = migrate_legacy_credentials(&conn, &web).unwrap();
        assert!(migrated);

        // Second migration attempt should skip.
        let migrated2 = migrate_legacy_credentials(&conn, &web).unwrap();
        assert!(!migrated2);
    }

    #[test]
    fn skips_when_empty_password() {
        let conn = test_conn();
        let web = WebConfig {
            dashboard_username: "admin".into(),
            dashboard_password: "".into(),
            ..Default::default()
        };
        let migrated = migrate_legacy_credentials(&conn, &web).unwrap();
        assert!(!migrated);
    }
}
