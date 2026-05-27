// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Persistent per-user login-attempt audit table (plan-02 AC-3).
//!
//! Every authentication attempt that resolves to a known `user_id` is
//! recorded here. The web layer reads `failed_count_in_window` between
//! username lookup and password verification to enforce per-user lockout
//! (≥ 5 failures in 10 minutes), and `clear_attempts` on admin override.
//!
//! Attempts against an **unknown** username are NOT recorded — otherwise
//! the 429 / 401 boundary would let an attacker enumerate valid usernames.
//!
//! ### Plan-11 step 3 worked example
//!
//! This is the first DAO migrated to the backend-agnostic [`DbAdapter`]
//! API (caller mandate, 2026-05-27). All five helpers are `async` and
//! take `&DbAdapter`; SQL uses `?` placeholders (sqlx rewrites to `$N`
//! for Postgres). The migration pattern here is the template the
//! follow-on DAOs follow:
//!   • take `&DbAdapter` (not `&rusqlite::Connection`);
//!   • return `Result<T>` (the crate-wide envelope; `?` propagates the
//!     adapter's `sqlx::Error` through `MaestroError::Sqlx`);
//!   • use the typed `DbValue` constructors for bind params;
//!   • do NOT mention `sqlx::sqlite::SqlitePool` / `PgPool` / `MySqlPool`.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::db::{DbAdapter, DbValue};
use crate::error::Result;

/// Kind of login attempt — distinguishes a password login from a recovery-code
/// reset attempt. Stored as the literal string `"password"` or `"recovery"`
/// (constrained by a CHECK on the column).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptKind {
    Password,
    Recovery,
}

impl AttemptKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AttemptKind::Password => "password",
            AttemptKind::Recovery => "recovery",
        }
    }
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Record a single authentication attempt for `user_id`.
///
/// `success = true` rows are kept so the lockout-clear path (G/W/T 3.6) can
/// distinguish "5 failures in the last 10 min" from "5 attempts including a
/// successful one in the last 10 min".
pub async fn record_attempt(
    adapter: &DbAdapter,
    user_id: &str,
    kind: AttemptKind,
    success: bool,
) -> Result<()> {
    adapter
        .execute(
            "INSERT INTO login_attempts (user_id, kind, attempted_at, success) \
             VALUES (?, ?, ?, ?)",
            vec![
                DbValue::Text(user_id.to_string()),
                DbValue::Text(kind.as_str().to_string()),
                DbValue::I64(now_unix()),
                DbValue::I64(if success { 1 } else { 0 }),
            ],
        )
        .await?;
    Ok(())
}

/// Count failed attempts (`success = 0`) for `(user_id, kind)` whose
/// `attempted_at` is within the last `window_secs` seconds.
pub async fn failed_count_in_window(
    adapter: &DbAdapter,
    user_id: &str,
    kind: AttemptKind,
    window_secs: i64,
) -> Result<i64> {
    let cutoff = now_unix() - window_secs;
    let row = adapter
        .query_one(
            "SELECT COUNT(*) FROM login_attempts \
             WHERE user_id = ? AND kind = ? AND success = 0 AND attempted_at > ?",
            vec![
                DbValue::Text(user_id.to_string()),
                DbValue::Text(kind.as_str().to_string()),
                DbValue::I64(cutoff),
            ],
        )
        .await?;
    Ok(row.get_i64(0)?)
}

/// Return the timestamp of the **oldest** failed attempt for `(user_id, kind)`
/// within the last `window_secs` seconds, or `None` if there are no failures.
///
/// Used to compute the `Retry-After` hint: `oldest + window - now`.
pub async fn oldest_failure_ts_in_window(
    adapter: &DbAdapter,
    user_id: &str,
    kind: AttemptKind,
    window_secs: i64,
) -> Result<Option<i64>> {
    let cutoff = now_unix() - window_secs;
    let row = adapter
        .query_one(
            "SELECT MIN(attempted_at) FROM login_attempts \
             WHERE user_id = ? AND kind = ? AND success = 0 AND attempted_at > ?",
            vec![
                DbValue::Text(user_id.to_string()),
                DbValue::Text(kind.as_str().to_string()),
                DbValue::I64(cutoff),
            ],
        )
        .await?;
    // MIN() over an empty set returns SQL NULL.
    Ok(row.get_i64_opt(0)?)
}

/// Delete every login_attempts row for `user_id` (both kinds).
///
/// Called by the admin "unlock" endpoint and after a successful login so the
/// counter resets cleanly rather than aging out via the window.
pub async fn clear_attempts(adapter: &DbAdapter, user_id: &str) -> Result<()> {
    adapter
        .execute(
            "DELETE FROM login_attempts WHERE user_id = ?",
            vec![DbValue::Text(user_id.to_string())],
        )
        .await?;
    Ok(())
}

/// Delete every **failed** login_attempts row for `(user_id, kind)`.
///
/// Used after a successful login so the persistent counter resets even though
/// `record_attempt(success=true)` has already inserted its success row.
pub async fn clear_failed_attempts(
    adapter: &DbAdapter,
    user_id: &str,
    kind: AttemptKind,
) -> Result<()> {
    adapter
        .execute(
            "DELETE FROM login_attempts WHERE user_id = ? AND kind = ? AND success = 0",
            vec![
                DbValue::Text(user_id.to_string()),
                DbValue::Text(kind.as_str().to_string()),
            ],
        )
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::adapter::DbAdapter;
    use crate::db::migrate::DialectAwareMigrationSource;
    use crate::db::pool::{DbBackend, DbPool};
    use sqlx::sqlite::SqlitePool;

    /// Plan-11 step 3 test pattern: build a fresh in-memory SQLite
    /// pool, apply the portable migration set, wrap in an adapter,
    /// then seed the foreign-key target (users row) so login_attempts
    /// inserts don't fail the FK constraint.
    ///
    /// This pattern will be the template for every migrated DAO's
    /// test setup. When the test rig in plan §10 step 4 lands,
    /// parametrising over `DbBackend::{Sqlite,Postgres,MySql}` is a
    /// one-line change at the top of this helper.
    async fn test_adapter_with_user() -> (DbAdapter, String) {
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
        let adapter = DbAdapter::new(DbPool::Sqlite(pool));

        // Seed the FK target. The users DAO is still on rusqlite, so we
        // INSERT directly via the adapter rather than going through it.
        adapter
            .execute(
                "INSERT INTO users (id, username, role) VALUES (?, ?, ?)",
                vec![
                    DbValue::Text("u-alice".into()),
                    DbValue::Text("alice".into()),
                    DbValue::Text("user".into()),
                ],
            )
            .await
            .expect("seed user");
        (adapter, "u-alice".to_string())
    }

    #[tokio::test]
    async fn record_and_count_failures() {
        let (a, uid) = test_adapter_with_user().await;

        for _ in 0..3 {
            record_attempt(&a, &uid, AttemptKind::Password, false)
                .await
                .unwrap();
        }
        let count = failed_count_in_window(&a, &uid, AttemptKind::Password, 600)
            .await
            .unwrap();
        assert_eq!(count, 3);
    }

    #[tokio::test]
    async fn success_attempts_do_not_count_as_failures() {
        let (a, uid) = test_adapter_with_user().await;

        record_attempt(&a, &uid, AttemptKind::Password, false)
            .await
            .unwrap();
        record_attempt(&a, &uid, AttemptKind::Password, true)
            .await
            .unwrap();
        record_attempt(&a, &uid, AttemptKind::Password, false)
            .await
            .unwrap();

        let count = failed_count_in_window(&a, &uid, AttemptKind::Password, 600)
            .await
            .unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn kind_filter_is_respected() {
        let (a, uid) = test_adapter_with_user().await;

        record_attempt(&a, &uid, AttemptKind::Password, false)
            .await
            .unwrap();
        record_attempt(&a, &uid, AttemptKind::Password, false)
            .await
            .unwrap();
        record_attempt(&a, &uid, AttemptKind::Recovery, false)
            .await
            .unwrap();

        assert_eq!(
            failed_count_in_window(&a, &uid, AttemptKind::Password, 600)
                .await
                .unwrap(),
            2
        );
        assert_eq!(
            failed_count_in_window(&a, &uid, AttemptKind::Recovery, 600)
                .await
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn window_filter_excludes_old_attempts() {
        let (a, uid) = test_adapter_with_user().await;

        // Manually insert an old failure ~20 minutes ago via the same
        // adapter (raw SQL is fine — this is test setup, not production).
        let now = now_unix();
        a.execute(
            "INSERT INTO login_attempts (user_id, kind, attempted_at, success) \
             VALUES (?, ?, ?, 0)",
            vec![
                DbValue::Text(uid.clone()),
                DbValue::Text("password".into()),
                DbValue::I64(now - 1200),
            ],
        )
        .await
        .unwrap();
        // And a recent one.
        record_attempt(&a, &uid, AttemptKind::Password, false)
            .await
            .unwrap();

        // Only the recent one is within a 10-minute window.
        let count = failed_count_in_window(&a, &uid, AttemptKind::Password, 600)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn clear_attempts_drops_all_rows() {
        let (a, uid) = test_adapter_with_user().await;

        record_attempt(&a, &uid, AttemptKind::Password, false)
            .await
            .unwrap();
        record_attempt(&a, &uid, AttemptKind::Recovery, false)
            .await
            .unwrap();
        clear_attempts(&a, &uid).await.unwrap();

        assert_eq!(
            failed_count_in_window(&a, &uid, AttemptKind::Password, 600)
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            failed_count_in_window(&a, &uid, AttemptKind::Recovery, 600)
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn clear_failed_attempts_keeps_success_rows() {
        let (a, uid) = test_adapter_with_user().await;

        record_attempt(&a, &uid, AttemptKind::Password, false)
            .await
            .unwrap();
        record_attempt(&a, &uid, AttemptKind::Password, true)
            .await
            .unwrap();
        clear_failed_attempts(&a, &uid, AttemptKind::Password)
            .await
            .unwrap();

        // Failed counter is cleared.
        assert_eq!(
            failed_count_in_window(&a, &uid, AttemptKind::Password, 600)
                .await
                .unwrap(),
            0
        );
        // But the success row is still in the table.
        let row = a
            .query_one(
                "SELECT COUNT(*) FROM login_attempts WHERE user_id = ?",
                vec![DbValue::Text(uid)],
            )
            .await
            .unwrap();
        assert_eq!(row.get_i64(0).unwrap(), 1);
    }

    #[tokio::test]
    async fn oldest_failure_ts_in_window_returns_min_ts() {
        let (a, uid) = test_adapter_with_user().await;
        let now = now_unix();
        // Two failures within window (200 s ago and 50 s ago) plus a success.
        for (ts_offset, success) in [(-200_i64, 0_i64), (-50, 0), (-10, 1)] {
            a.execute(
                "INSERT INTO login_attempts (user_id, kind, attempted_at, success) \
                 VALUES (?, ?, ?, ?)",
                vec![
                    DbValue::Text(uid.clone()),
                    DbValue::Text("password".into()),
                    DbValue::I64(now + ts_offset),
                    DbValue::I64(success),
                ],
            )
            .await
            .unwrap();
        }

        let oldest = oldest_failure_ts_in_window(&a, &uid, AttemptKind::Password, 600)
            .await
            .unwrap()
            .expect("expected a Some(oldest_ts)");
        assert_eq!(oldest, now - 200);

        // Empty case → None.
        clear_attempts(&a, &uid).await.unwrap();
        assert!(
            oldest_failure_ts_in_window(&a, &uid, AttemptKind::Password, 600)
                .await
                .unwrap()
                .is_none()
        );
    }
}
