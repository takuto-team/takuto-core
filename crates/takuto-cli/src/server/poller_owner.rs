// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Resolve which user owns poller-created workflows at startup.

use tracing::info;

use takuto_core::db::Database;

/// Resolve the owner of poller-created workflows at startup.
///
/// Resolution order:
///   1. If `cfg_username` is provided AND the user exists AND is not suspended, return their id.
///   2. Otherwise, return the id of the lexicographically-first non-suspended admin.
///   3. Otherwise, return `None` (poller will skip `start_workflow` and log).
///
/// Warnings are logged when (1) is provided but the user is missing or suspended,
/// and when neither path resolves (the caller may log an additional summary).
pub(crate) async fn resolve_poller_owner(
    db: &Database,
    cfg_username: Option<&str>,
) -> Option<String> {
    let adapter = db.adapter();
    if let Some(username) = cfg_username {
        match takuto_core::db::users::get_user_by_username(adapter, username).await {
            Ok(Some(user)) if !user.suspended => {
                info!(
                    username = %user.username,
                    user_id = %user.id,
                    "Poller owner resolved from [general] poller_owner_username"
                );
                return Some(user.id);
            }
            Ok(Some(user)) => {
                tracing::warn!(
                    username = %username,
                    user_id = %user.id,
                    "Configured poller_owner_username is suspended; falling back to admin"
                );
            }
            Ok(None) => {
                tracing::warn!(
                    username = %username,
                    "Configured poller_owner_username not found; falling back to admin"
                );
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    username = %username,
                    "Lookup for poller_owner_username failed; falling back to admin"
                );
            }
        }
    }

    match takuto_core::db::users::list_admins(adapter).await {
        Ok(admins) => admins.into_iter().next().map(|u| {
            info!(
                username = %u.username,
                user_id = %u.id,
                "Poller owner resolved to first non-suspended admin"
            );
            u.id
        }),
        Err(e) => {
            tracing::warn!(error = %e, "Failed to list admins for poller-owner resolution");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_poller_owner;
    use takuto_core::db::{Database, DbValue};

    /// A migrated temp-dir SQLite database (the cross-crate test-DB idiom —
    /// `open_in_memory` is test-only inside takuto-core). The returned
    /// `TempDir` guard must be kept alive for the DB's lifetime.
    fn temp_db() -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path(), true).expect("open temp db");
        (dir, db)
    }

    async fn seed_user(db: &Database, username: &str, role: &str, suspended: bool) -> String {
        let id = format!("u-{username}");
        db.adapter()
            .execute(
                "INSERT INTO users (id, username, role, suspended) VALUES (?, ?, ?, ?)",
                vec![
                    DbValue::Text(id.clone()),
                    DbValue::Text(username.to_string()),
                    DbValue::Text(role.to_string()),
                    DbValue::I64(i64::from(suspended)),
                ],
            )
            .await
            .unwrap();
        id
    }

    #[tokio::test]
    async fn resolves_configured_username_when_active() {
        let (_dir, db) = temp_db();
        let alice = seed_user(&db, "alice", "user", false).await;
        seed_user(&db, "zadmin", "admin", false).await;
        let owner = resolve_poller_owner(&db, Some("alice")).await;
        assert_eq!(owner.as_deref(), Some(alice.as_str()));
    }

    #[tokio::test]
    async fn falls_back_to_admin_when_configured_user_missing() {
        let (_dir, db) = temp_db();
        let admin = seed_user(&db, "admin", "admin", false).await;
        let owner = resolve_poller_owner(&db, Some("ghost")).await;
        assert_eq!(owner.as_deref(), Some(admin.as_str()));
    }

    #[tokio::test]
    async fn falls_back_to_admin_when_configured_user_suspended() {
        let (_dir, db) = temp_db();
        seed_user(&db, "alice", "user", true).await;
        let admin = seed_user(&db, "admin", "admin", false).await;
        let owner = resolve_poller_owner(&db, Some("alice")).await;
        assert_eq!(owner.as_deref(), Some(admin.as_str()));
    }

    #[tokio::test]
    async fn admin_fallback_without_configured_username() {
        let (_dir, db) = temp_db();
        let admin = seed_user(&db, "admin", "admin", false).await;
        let owner = resolve_poller_owner(&db, None).await;
        assert_eq!(owner.as_deref(), Some(admin.as_str()));
    }

    #[tokio::test]
    async fn none_when_no_admin_and_no_match() {
        let (_dir, db) = temp_db();
        seed_user(&db, "alice", "user", false).await;
        let owner = resolve_poller_owner(&db, Some("ghost")).await;
        assert_eq!(owner, None);
    }
}
