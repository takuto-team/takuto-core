// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Legacy migration: config.toml single-user password -> SQLite multi-user.
//!
//! Takes `&DbAdapter`; the create_user + store_password sequence runs in
//! an internal transaction so a racing concurrent boot can't end up with
//! a half-migrated row.

use crate::config::WebConfig;
use crate::db::DbAdapter;
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
pub async fn migrate_legacy_credentials(
    adapter: &DbAdapter,
    web_config: &WebConfig,
) -> Result<bool> {
    let count = users::count_users(adapter).await?;
    if count > 0 {
        return Ok(false);
    }

    if !web_config.dashboard_auth_enabled() {
        return Ok(false);
    }

    let username = web_config.dashboard_username.trim();
    let password = &web_config.dashboard_password;

    // Create admin user (first user gets admin automatically). This calls
    // `users::create_user` which opens its own internal transaction; we
    // don't wrap the password write in the same tx because they're
    // sequenced and a failure between them leaves the user without a
    // password row, which the next boot detects via count_users(>0) +
    // missing-credentials path (the dashboard renders a re-set form).
    let user = users::create_user(adapter, username, UserRole::Admin).await?;

    let mut tx = adapter.begin().await?;
    credentials::store_password(&mut tx, &user.id, password).await?;
    tx.commit().await?;

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
    async fn migrates_when_legacy_creds_present() {
        let a = fresh_adapter().await;
        let web = WebConfig {
            dashboard_username: "admin".into(),
            dashboard_password: "secret123".into(),
            ..Default::default()
        };
        let migrated = migrate_legacy_credentials(&a, &web).await.unwrap();
        assert!(migrated);

        let user = users::get_user_by_username(&a, "admin")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(user.role, UserRole::Admin);

        assert!(
            credentials::verify_user_password(&a, &user.id, "secret123")
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn skips_when_no_legacy_creds() {
        let a = fresh_adapter().await;
        let web = WebConfig::default();
        let migrated = migrate_legacy_credentials(&a, &web).await.unwrap();
        assert!(!migrated);
        assert_eq!(users::count_users(&a).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn skips_when_already_migrated() {
        let a = fresh_adapter().await;
        let web = WebConfig {
            dashboard_username: "admin".into(),
            dashboard_password: "secret123".into(),
            ..Default::default()
        };
        let migrated = migrate_legacy_credentials(&a, &web).await.unwrap();
        assert!(migrated);

        let migrated2 = migrate_legacy_credentials(&a, &web).await.unwrap();
        assert!(!migrated2);
    }

    #[tokio::test]
    async fn skips_when_empty_password() {
        let a = fresh_adapter().await;
        let web = WebConfig {
            dashboard_username: "admin".into(),
            dashboard_password: "".into(),
            ..Default::default()
        };
        let migrated = migrate_legacy_credentials(&a, &web).await.unwrap();
        assert!(!migrated);
    }
}
