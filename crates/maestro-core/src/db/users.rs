// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! User CRUD operations against the SQLite database.

use rusqlite::params;
use uuid::Uuid;

use crate::auth::AuthError;
use crate::error::Result;

use super::models::{User, UserRole};

/// Create a new user. Returns the created user.
///
/// If no users exist in the database, the first user is automatically assigned the `admin` role
/// regardless of the `role` parameter (first-user-becomes-admin).
pub fn create_user(conn: &rusqlite::Connection, username: &str, role: UserRole) -> Result<User> {
    let username = username.trim();
    if username.is_empty() {
        return Err(AuthError::EmptyUsername.into());
    }

    // First-user-becomes-admin: if no users exist, force admin role.
    // Wrapped in a transaction for atomicity (race condition guard).
    let tx = conn.unchecked_transaction()?;

    let count: i64 = tx.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))?;
    let effective_role = if count == 0 { UserRole::Admin } else { role };

    let id = Uuid::new_v4().to_string();
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

    tx.execute(
        "INSERT INTO users (id, username, role, suspended, created_at, updated_at) \
         VALUES (?1, ?2, ?3, 0, ?4, ?5)",
        params![id, username, effective_role.as_str(), now, now],
    )
    .map_err(|e| -> crate::error::MaestroError {
        if let rusqlite::Error::SqliteFailure(ref err, _) = e
            && err.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
        {
            return AuthError::UsernameAlreadyExists {
                username: username.to_string(),
            }
            .into();
        }
        super::DbError::Sqlite(e).into()
    })?;

    tx.commit()?;

    Ok(User {
        id,
        username: username.to_string(),
        role: effective_role,
        suspended: false,
        created_at: now.clone(),
        updated_at: now,
    })
}

/// Get a user by their ID.
pub fn get_user_by_id(conn: &rusqlite::Connection, id: &str) -> Result<Option<User>> {
    let mut stmt = conn.prepare(
        "SELECT id, username, role, suspended, created_at, updated_at FROM users WHERE id = ?1",
    )?;

    let user = stmt.query_row(params![id], row_to_user).optional()?;

    Ok(user)
}

/// Get a user by their username.
pub fn get_user_by_username(conn: &rusqlite::Connection, username: &str) -> Result<Option<User>> {
    let mut stmt = conn.prepare(
        "SELECT id, username, role, suspended, created_at, updated_at FROM users WHERE username = ?1",
    )?;

    let user = stmt
        .query_row(params![username.trim()], row_to_user)
        .optional()?;

    Ok(user)
}

/// List all users, ordered by creation date.
pub fn list_users(conn: &rusqlite::Connection) -> Result<Vec<User>> {
    let mut stmt = conn.prepare(
        "SELECT id, username, role, suspended, created_at, updated_at FROM users ORDER BY created_at ASC",
    )?;

    let users = stmt
        .query_map([], row_to_user)?
        .filter_map(|r| r.ok())
        .collect();

    Ok(users)
}

/// List all non-suspended admins, ordered by username (ascending). Used at startup
/// by `crates/maestro-cli/src/main.rs::resolve_poller_owner` to pick a deterministic
/// fallback owner for poller-created workflows.
pub fn list_admins(conn: &rusqlite::Connection) -> Result<Vec<User>> {
    let mut stmt = conn.prepare(
        "SELECT id, username, role, suspended, created_at, updated_at \
         FROM users \
         WHERE role = 'admin' AND suspended = 0 \
         ORDER BY username ASC",
    )?;

    let admins = stmt
        .query_map([], row_to_user)?
        .filter_map(|r| r.ok())
        .collect();

    Ok(admins)
}

/// Update a user's username and/or role. Returns the updated user.
pub fn update_user(
    conn: &rusqlite::Connection,
    id: &str,
    new_username: Option<&str>,
    new_role: Option<UserRole>,
) -> Result<User> {
    let existing = get_user_by_id(conn, id)?.ok_or_else(|| AuthError::UserNotFound {
        id: id.to_string(),
    })?;

    // If demoting the last non-suspended admin, reject.
    if let Some(new_role) = new_role
        && existing.role == UserRole::Admin
        && new_role == UserRole::User
    {
        let admin_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM users WHERE role = 'admin' AND suspended = 0 AND id != ?1",
            params![id],
            |r| r.get(0),
        )?;
        if admin_count == 0 {
            return Err(AuthError::LastAdminLockout { op: "demote" }.into());
        }
    }

    let username = new_username.map(|u| u.trim()).unwrap_or(&existing.username);
    let role = new_role.unwrap_or(existing.role);
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

    conn.execute(
        "UPDATE users SET username = ?1, role = ?2, updated_at = ?3 WHERE id = ?4",
        params![username, role.as_str(), now, id],
    )?;

    get_user_by_id(conn, id)?
        .ok_or_else(|| AuthError::UserDisappearedAfterUpdate.into())
}

/// Suspend a user. Fails if this would leave zero non-suspended admins.
pub fn suspend_user(conn: &rusqlite::Connection, id: &str) -> Result<()> {
    let user = get_user_by_id(conn, id)?.ok_or_else(|| AuthError::UserNotFound {
        id: id.to_string(),
    })?;

    if user.role == UserRole::Admin {
        let admin_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM users WHERE role = 'admin' AND suspended = 0 AND id != ?1",
            params![id],
            |r| r.get(0),
        )?;
        if admin_count == 0 {
            return Err(AuthError::LastAdminLockout { op: "suspend" }.into());
        }
    }

    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    conn.execute(
        "UPDATE users SET suspended = 1, updated_at = ?1 WHERE id = ?2",
        params![now, id],
    )?;
    Ok(())
}

/// Unsuspend a user.
pub fn unsuspend_user(conn: &rusqlite::Connection, id: &str) -> Result<()> {
    let exists: bool = conn.query_row(
        "SELECT COUNT(*) > 0 FROM users WHERE id = ?1",
        params![id],
        |r| r.get(0),
    )?;
    if !exists {
        return Err(AuthError::UserNotFound {
            id: id.to_string(),
        }
        .into());
    }

    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    conn.execute(
        "UPDATE users SET suspended = 0, updated_at = ?1 WHERE id = ?2",
        params![now, id],
    )?;
    Ok(())
}

/// Delete a user and all associated data (cascading FK). Fails if last non-suspended admin.
pub fn delete_user(conn: &rusqlite::Connection, id: &str) -> Result<()> {
    let user = get_user_by_id(conn, id)?.ok_or_else(|| AuthError::UserNotFound {
        id: id.to_string(),
    })?;

    if user.role == UserRole::Admin {
        let admin_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM users WHERE role = 'admin' AND suspended = 0 AND id != ?1",
            params![id],
            |r| r.get(0),
        )?;
        if admin_count == 0 {
            return Err(AuthError::LastAdminLockout { op: "delete" }.into());
        }
    }

    conn.execute("DELETE FROM users WHERE id = ?1", params![id])?;
    Ok(())
}

/// Count total users (for first-user-becomes-admin check).
pub fn count_users(conn: &rusqlite::Connection) -> Result<i64> {
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))?;
    Ok(count)
}

/// Helper: convert a row to a [`User`].
fn row_to_user(row: &rusqlite::Row<'_>) -> rusqlite::Result<User> {
    let role_str: String = row.get(2)?;
    let suspended_int: i32 = row.get(3)?;
    Ok(User {
        id: row.get(0)?,
        username: row.get(1)?,
        role: role_str.parse().unwrap_or(UserRole::User),
        suspended: suspended_int != 0,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

/// Extension trait for optional query results.
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
    use crate::db::schema;

    fn test_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        schema::run_migrations(&conn).unwrap();
        conn
    }

    #[test]
    fn first_user_becomes_admin() {
        let conn = test_conn();
        let user = create_user(&conn, "alice", UserRole::User).unwrap();
        assert_eq!(user.role, UserRole::Admin);
        assert_eq!(user.username, "alice");
    }

    #[test]
    fn second_user_gets_requested_role() {
        let conn = test_conn();
        let _ = create_user(&conn, "alice", UserRole::User).unwrap();
        let bob = create_user(&conn, "bob", UserRole::User).unwrap();
        assert_eq!(bob.role, UserRole::User);
    }

    #[test]
    fn duplicate_username_rejected() {
        let conn = test_conn();
        let _ = create_user(&conn, "alice", UserRole::User).unwrap();
        let err = create_user(&conn, "alice", UserRole::User);
        assert!(err.is_err());
        let msg = err.unwrap_err().to_string();
        assert!(msg.contains("already exists"), "got: {msg}");
    }

    #[test]
    fn empty_username_rejected() {
        let conn = test_conn();
        let err = create_user(&conn, "  ", UserRole::User);
        assert!(err.is_err());
    }

    #[test]
    fn get_user_by_id_found() {
        let conn = test_conn();
        let user = create_user(&conn, "alice", UserRole::User).unwrap();
        let found = get_user_by_id(&conn, &user.id).unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().username, "alice");
    }

    #[test]
    fn get_user_by_id_not_found() {
        let conn = test_conn();
        let found = get_user_by_id(&conn, "nonexistent").unwrap();
        assert!(found.is_none());
    }

    #[test]
    fn get_user_by_username_found() {
        let conn = test_conn();
        let _ = create_user(&conn, "alice", UserRole::User).unwrap();
        let found = get_user_by_username(&conn, "alice").unwrap();
        assert!(found.is_some());
    }

    #[test]
    fn list_users_returns_all() {
        let conn = test_conn();
        let _ = create_user(&conn, "alice", UserRole::User).unwrap();
        let _ = create_user(&conn, "bob", UserRole::User).unwrap();
        let users = list_users(&conn).unwrap();
        assert_eq!(users.len(), 2);
    }

    #[test]
    fn update_user_changes_username() {
        let conn = test_conn();
        let user = create_user(&conn, "alice", UserRole::User).unwrap();
        let _ = create_user(&conn, "bob", UserRole::User).unwrap(); // second user so alice isn't the only admin
        let updated = update_user(&conn, &user.id, Some("alice2"), None).unwrap();
        assert_eq!(updated.username, "alice2");
    }

    #[test]
    fn cannot_demote_last_admin() {
        let conn = test_conn();
        let admin = create_user(&conn, "alice", UserRole::User).unwrap(); // first user = admin
        assert_eq!(admin.role, UserRole::Admin);
        let err = update_user(&conn, &admin.id, None, Some(UserRole::User));
        assert!(err.is_err());
        assert!(
            err.unwrap_err()
                .to_string()
                .contains("last non-suspended admin")
        );
    }

    #[test]
    fn suspend_and_unsuspend() {
        let conn = test_conn();
        let admin = create_user(&conn, "alice", UserRole::User).unwrap();
        let bob = create_user(&conn, "bob", UserRole::Admin).unwrap(); // another admin
        let _ = bob; // ensure we have 2 admins

        // Actually alice is admin (first user). Let's suspend bob instead.
        suspend_user(&conn, &bob.id).unwrap();
        let suspended = get_user_by_id(&conn, &bob.id).unwrap().unwrap();
        assert!(suspended.suspended);

        unsuspend_user(&conn, &bob.id).unwrap();
        let unsuspended = get_user_by_id(&conn, &bob.id).unwrap().unwrap();
        assert!(!unsuspended.suspended);

        let _ = admin; // prevent unused var warning
    }

    #[test]
    fn cannot_suspend_last_admin() {
        let conn = test_conn();
        let admin = create_user(&conn, "alice", UserRole::User).unwrap();
        assert_eq!(admin.role, UserRole::Admin);
        let err = suspend_user(&conn, &admin.id);
        assert!(err.is_err());
        assert!(
            err.unwrap_err()
                .to_string()
                .contains("last non-suspended admin")
        );
    }

    #[test]
    fn delete_user_cascades() {
        let conn = test_conn();
        let admin = create_user(&conn, "alice", UserRole::User).unwrap();
        let bob = create_user(&conn, "bob", UserRole::User).unwrap();

        delete_user(&conn, &bob.id).unwrap();
        assert!(get_user_by_id(&conn, &bob.id).unwrap().is_none());

        // Admin still exists
        assert!(get_user_by_id(&conn, &admin.id).unwrap().is_some());
    }

    #[test]
    fn cannot_delete_last_admin() {
        let conn = test_conn();
        let admin = create_user(&conn, "alice", UserRole::User).unwrap();
        assert_eq!(admin.role, UserRole::Admin);
        let err = delete_user(&conn, &admin.id);
        assert!(err.is_err());
    }

    #[test]
    fn count_users_works() {
        let conn = test_conn();
        assert_eq!(count_users(&conn).unwrap(), 0);
        let _ = create_user(&conn, "alice", UserRole::User).unwrap();
        assert_eq!(count_users(&conn).unwrap(), 1);
    }
}
