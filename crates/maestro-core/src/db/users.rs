// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! User CRUD operations against the SQLite database.
//!
//! All fns are `async` over the agnostic [`DbAdapter`]; reads take
//! `&DbAdapter`, writes that need multi-statement atomicity (the
//! last-admin guards + the first-user-becomes-admin race in `create_user`)
//! open a short internal `DbTransaction` and commit before returning. No
//! external caller needs to manage the transaction.

use uuid::Uuid;

use crate::auth::AuthError;
use crate::db::{DbAdapter, DbValue};
use crate::error::Result;

use super::models::{User, UserRole};

fn now_iso() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

const SELECT_COLS: &str = "id, username, role, suspended, created_at, updated_at";

fn decode_user(r: &crate::db::DbRow) -> Result<User> {
    let role_str = r.get_text(2)?;
    let suspended = r.get_i64(3)? != 0;
    Ok(User {
        id: r.get_text(0)?,
        username: r.get_text(1)?,
        role: role_str.parse().unwrap_or(UserRole::User),
        suspended,
        created_at: r.get_text(4)?,
        updated_at: r.get_text(5)?,
    })
}

/// Create a new user. Returns the created user.
///
/// First-user-becomes-admin: if no users exist, the first user is
/// automatically assigned the `admin` role regardless of the `role`
/// parameter. The count-then-insert sequence runs inside one
/// `DbTransaction` so a concurrent second creator can't race past the
/// count check and end up with a non-admin first user.
pub async fn create_user(adapter: &DbAdapter, username: &str, role: UserRole) -> Result<User> {
    let username = username.trim();
    if username.is_empty() {
        return Err(AuthError::EmptyUsername.into());
    }

    let mut tx = adapter.begin().await?;
    let count_row = tx
        .query_one("SELECT COUNT(*) FROM users", vec![])
        .await?;
    let count = count_row.get_i64(0)?;
    let effective_role = if count == 0 { UserRole::Admin } else { role };

    let id = Uuid::new_v4().to_string();
    let now = now_iso();
    let result = tx
        .execute(
            "INSERT INTO users (id, username, role, suspended, created_at, updated_at) \
             VALUES (?, ?, ?, 0, ?, ?)",
            vec![
                DbValue::Text(id.clone()),
                DbValue::Text(username.to_string()),
                DbValue::Text(effective_role.as_str().to_string()),
                DbValue::Text(now.clone()),
                DbValue::Text(now.clone()),
            ],
        )
        .await;
    if let Err(e) = result {
        // Translate UNIQUE constraint into the typed Auth error. sqlx
        // surfaces SQLite constraint failures via `Error::Database`;
        // checking the message is the portable shape across backends.
        let msg = e.to_string();
        if msg.contains("UNIQUE") || msg.contains("unique constraint") {
            return Err(AuthError::UsernameAlreadyExists {
                username: username.to_string(),
            }
            .into());
        }
        return Err(e.into());
    }

    tx.commit().await?;

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
pub async fn get_user_by_id(adapter: &DbAdapter, id: &str) -> Result<Option<User>> {
    let sql = format!("SELECT {SELECT_COLS} FROM users WHERE id = ?");
    let row = adapter
        .query_optional(&sql, vec![DbValue::Text(id.to_string())])
        .await?;
    row.map(|r| decode_user(&r)).transpose()
}

/// Get a user by their username.
pub async fn get_user_by_username(adapter: &DbAdapter, username: &str) -> Result<Option<User>> {
    let sql = format!("SELECT {SELECT_COLS} FROM users WHERE username = ?");
    let row = adapter
        .query_optional(&sql, vec![DbValue::Text(username.trim().to_string())])
        .await?;
    row.map(|r| decode_user(&r)).transpose()
}

/// List all users, ordered by creation date.
pub async fn list_users(adapter: &DbAdapter) -> Result<Vec<User>> {
    let sql = format!("SELECT {SELECT_COLS} FROM users ORDER BY created_at ASC");
    let rows = adapter.query_all(&sql, vec![]).await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        out.push(decode_user(r)?);
    }
    Ok(out)
}

/// List all non-suspended admins, ordered by username (ascending).
pub async fn list_admins(adapter: &DbAdapter) -> Result<Vec<User>> {
    let sql = format!(
        "SELECT {SELECT_COLS} FROM users \
         WHERE role = 'admin' AND suspended = 0 \
         ORDER BY username ASC"
    );
    let rows = adapter.query_all(&sql, vec![]).await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        out.push(decode_user(r)?);
    }
    Ok(out)
}

/// Update a user's username and/or role. Returns the updated user.
///
/// Last-admin guard runs inside an internal transaction so a concurrent
/// admin demote can't slip past the count check.
pub async fn update_user(
    adapter: &DbAdapter,
    id: &str,
    new_username: Option<&str>,
    new_role: Option<UserRole>,
) -> Result<User> {
    let mut tx = adapter.begin().await?;

    let existing_row = tx
        .query_optional(
            &format!("SELECT {SELECT_COLS} FROM users WHERE id = ?"),
            vec![DbValue::Text(id.to_string())],
        )
        .await?;
    let existing = match existing_row {
        Some(r) => decode_user(&r)?,
        None => {
            return Err(AuthError::UserNotFound {
                id: id.to_string(),
            }
            .into());
        }
    };

    if let Some(new_role) = new_role
        && existing.role == UserRole::Admin
        && new_role == UserRole::User
    {
        let admin_count_row = tx
            .query_one(
                "SELECT COUNT(*) FROM users \
                 WHERE role = 'admin' AND suspended = 0 AND id != ?",
                vec![DbValue::Text(id.to_string())],
            )
            .await?;
        if admin_count_row.get_i64(0)? == 0 {
            return Err(AuthError::LastAdminLockout { op: "demote" }.into());
        }
    }

    let username = new_username.map(|u| u.trim()).unwrap_or(&existing.username);
    let role = new_role.unwrap_or(existing.role);
    let now = now_iso();

    tx.execute(
        "UPDATE users SET username = ?, role = ?, updated_at = ? WHERE id = ?",
        vec![
            DbValue::Text(username.to_string()),
            DbValue::Text(role.as_str().to_string()),
            DbValue::Text(now.clone()),
            DbValue::Text(id.to_string()),
        ],
    )
    .await?;

    // Re-read inside the same tx so the response reflects the committed state.
    let row = tx
        .query_optional(
            &format!("SELECT {SELECT_COLS} FROM users WHERE id = ?"),
            vec![DbValue::Text(id.to_string())],
        )
        .await?;
    tx.commit().await?;
    row.map(|r| decode_user(&r))
        .transpose()?
        .ok_or_else(|| AuthError::UserDisappearedAfterUpdate.into())
}

/// Suspend a user. Fails if this would leave zero non-suspended admins.
pub async fn suspend_user(adapter: &DbAdapter, id: &str) -> Result<()> {
    let mut tx = adapter.begin().await?;
    let row = tx
        .query_optional(
            &format!("SELECT {SELECT_COLS} FROM users WHERE id = ?"),
            vec![DbValue::Text(id.to_string())],
        )
        .await?;
    let user = match row {
        Some(r) => decode_user(&r)?,
        None => {
            return Err(AuthError::UserNotFound {
                id: id.to_string(),
            }
            .into());
        }
    };
    if user.role == UserRole::Admin {
        let admin_count_row = tx
            .query_one(
                "SELECT COUNT(*) FROM users \
                 WHERE role = 'admin' AND suspended = 0 AND id != ?",
                vec![DbValue::Text(id.to_string())],
            )
            .await?;
        if admin_count_row.get_i64(0)? == 0 {
            return Err(AuthError::LastAdminLockout { op: "suspend" }.into());
        }
    }
    tx.execute(
        "UPDATE users SET suspended = 1, updated_at = ? WHERE id = ?",
        vec![DbValue::Text(now_iso()), DbValue::Text(id.to_string())],
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Unsuspend a user.
pub async fn unsuspend_user(adapter: &DbAdapter, id: &str) -> Result<()> {
    let exists_row = adapter
        .query_one(
            "SELECT COUNT(*) > 0 FROM users WHERE id = ?",
            vec![DbValue::Text(id.to_string())],
        )
        .await?;
    let exists = exists_row.get_i64(0)? != 0;
    if !exists {
        return Err(AuthError::UserNotFound {
            id: id.to_string(),
        }
        .into());
    }
    adapter
        .execute(
            "UPDATE users SET suspended = 0, updated_at = ? WHERE id = ?",
            vec![DbValue::Text(now_iso()), DbValue::Text(id.to_string())],
        )
        .await?;
    Ok(())
}

/// Delete a user and all associated data (cascading FK). Fails if last
/// non-suspended admin. Wraps the count-then-delete check in one
/// transaction so a racing admin promote/suspend can't slip through.
pub async fn delete_user(adapter: &DbAdapter, id: &str) -> Result<()> {
    let mut tx = adapter.begin().await?;
    let row = tx
        .query_optional(
            &format!("SELECT {SELECT_COLS} FROM users WHERE id = ?"),
            vec![DbValue::Text(id.to_string())],
        )
        .await?;
    let user = match row {
        Some(r) => decode_user(&r)?,
        None => {
            return Err(AuthError::UserNotFound {
                id: id.to_string(),
            }
            .into());
        }
    };
    if user.role == UserRole::Admin {
        let admin_count_row = tx
            .query_one(
                "SELECT COUNT(*) FROM users \
                 WHERE role = 'admin' AND suspended = 0 AND id != ?",
                vec![DbValue::Text(id.to_string())],
            )
            .await?;
        if admin_count_row.get_i64(0)? == 0 {
            return Err(AuthError::LastAdminLockout { op: "delete" }.into());
        }
    }
    tx.execute(
        "DELETE FROM users WHERE id = ?",
        vec![DbValue::Text(id.to_string())],
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Count total users (for first-user-becomes-admin check).
pub async fn count_users(adapter: &DbAdapter) -> Result<i64> {
    let row = adapter
        .query_one("SELECT COUNT(*) FROM users", vec![])
        .await?;
    Ok(row.get_i64(0)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::adapter::DbAdapter;
    use crate::db::migrate::DialectAwareMigrationSource;
    use crate::db::pool::{DbBackend, DbPool};
    use sqlx::sqlite::SqlitePool;

    async fn fresh_adapter() -> DbAdapter {
        let pool = SqlitePool::connect("sqlite::memory:")
            .await
            .expect("connect");
        let source = DialectAwareMigrationSource::for_backend(DbBackend::Sqlite);
        sqlx::migrate::Migrator::new(source)
            .await
            .unwrap()
            .run(&pool)
            .await
            .unwrap();
        DbAdapter::new(DbPool::Sqlite(pool))
    }

    #[tokio::test]
    async fn first_user_becomes_admin() {
        let a = fresh_adapter().await;
        let user = create_user(&a, "alice", UserRole::User).await.unwrap();
        assert_eq!(user.role, UserRole::Admin);
        assert_eq!(user.username, "alice");
    }

    #[tokio::test]
    async fn second_user_gets_requested_role() {
        let a = fresh_adapter().await;
        let _ = create_user(&a, "alice", UserRole::User).await.unwrap();
        let bob = create_user(&a, "bob", UserRole::User).await.unwrap();
        assert_eq!(bob.role, UserRole::User);
    }

    #[tokio::test]
    async fn duplicate_username_rejected() {
        let a = fresh_adapter().await;
        let _ = create_user(&a, "alice", UserRole::User).await.unwrap();
        let err = create_user(&a, "alice", UserRole::User).await;
        assert!(err.is_err());
        let msg = err.unwrap_err().to_string();
        assert!(msg.contains("already exists"), "got: {msg}");
    }

    #[tokio::test]
    async fn empty_username_rejected() {
        let a = fresh_adapter().await;
        let err = create_user(&a, "  ", UserRole::User).await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn get_user_by_id_found() {
        let a = fresh_adapter().await;
        let user = create_user(&a, "alice", UserRole::User).await.unwrap();
        let found = get_user_by_id(&a, &user.id).await.unwrap();
        assert!(found.is_some());
        assert_eq!(found.unwrap().username, "alice");
    }

    #[tokio::test]
    async fn get_user_by_id_not_found() {
        let a = fresh_adapter().await;
        let found = get_user_by_id(&a, "nonexistent").await.unwrap();
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn get_user_by_username_found() {
        let a = fresh_adapter().await;
        let _ = create_user(&a, "alice", UserRole::User).await.unwrap();
        let found = get_user_by_username(&a, "alice").await.unwrap();
        assert!(found.is_some());
    }

    #[tokio::test]
    async fn list_users_returns_all() {
        let a = fresh_adapter().await;
        let _ = create_user(&a, "alice", UserRole::User).await.unwrap();
        let _ = create_user(&a, "bob", UserRole::User).await.unwrap();
        let users = list_users(&a).await.unwrap();
        assert_eq!(users.len(), 2);
    }

    #[tokio::test]
    async fn update_user_changes_username() {
        let a = fresh_adapter().await;
        let user = create_user(&a, "alice", UserRole::User).await.unwrap();
        let _ = create_user(&a, "bob", UserRole::User).await.unwrap();
        let updated = update_user(&a, &user.id, Some("alice2"), None).await.unwrap();
        assert_eq!(updated.username, "alice2");
    }

    #[tokio::test]
    async fn cannot_demote_last_admin() {
        let a = fresh_adapter().await;
        let admin = create_user(&a, "alice", UserRole::User).await.unwrap();
        assert_eq!(admin.role, UserRole::Admin);
        let err = update_user(&a, &admin.id, None, Some(UserRole::User)).await;
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("last non-suspended admin"));
    }

    #[tokio::test]
    async fn suspend_and_unsuspend() {
        let a = fresh_adapter().await;
        let admin = create_user(&a, "alice", UserRole::User).await.unwrap();
        let bob = create_user(&a, "bob", UserRole::Admin).await.unwrap();
        let _ = bob.clone();

        suspend_user(&a, &bob.id).await.unwrap();
        let suspended = get_user_by_id(&a, &bob.id).await.unwrap().unwrap();
        assert!(suspended.suspended);

        unsuspend_user(&a, &bob.id).await.unwrap();
        let unsuspended = get_user_by_id(&a, &bob.id).await.unwrap().unwrap();
        assert!(!unsuspended.suspended);

        let _ = admin;
    }

    #[tokio::test]
    async fn cannot_suspend_last_admin() {
        let a = fresh_adapter().await;
        let admin = create_user(&a, "alice", UserRole::User).await.unwrap();
        assert_eq!(admin.role, UserRole::Admin);
        let err = suspend_user(&a, &admin.id).await;
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("last non-suspended admin"));
    }

    #[tokio::test]
    async fn delete_user_cascades() {
        let a = fresh_adapter().await;
        let admin = create_user(&a, "alice", UserRole::User).await.unwrap();
        let bob = create_user(&a, "bob", UserRole::User).await.unwrap();

        delete_user(&a, &bob.id).await.unwrap();
        assert!(get_user_by_id(&a, &bob.id).await.unwrap().is_none());

        assert!(get_user_by_id(&a, &admin.id).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn cannot_delete_last_admin() {
        let a = fresh_adapter().await;
        let admin = create_user(&a, "alice", UserRole::User).await.unwrap();
        assert_eq!(admin.role, UserRole::Admin);
        let err = delete_user(&a, &admin.id).await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn count_users_works() {
        let a = fresh_adapter().await;
        assert_eq!(count_users(&a).await.unwrap(), 0);
        let _ = create_user(&a, "alice", UserRole::User).await.unwrap();
        assert_eq!(count_users(&a).await.unwrap(), 1);
    }
}
