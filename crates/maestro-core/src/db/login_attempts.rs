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

use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::params;

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
pub fn record_attempt(
    conn: &rusqlite::Connection,
    user_id: &str,
    kind: AttemptKind,
    success: bool,
) -> Result<()> {
    conn.execute(
        "INSERT INTO login_attempts (user_id, kind, attempted_at, success) VALUES (?1, ?2, ?3, ?4)",
        params![
            user_id,
            kind.as_str(),
            now_unix(),
            if success { 1 } else { 0 }
        ],
    )?;
    Ok(())
}

/// Count failed attempts (`success = 0`) for `(user_id, kind)` whose
/// `attempted_at` is within the last `window_secs` seconds.
pub fn failed_count_in_window(
    conn: &rusqlite::Connection,
    user_id: &str,
    kind: AttemptKind,
    window_secs: i64,
) -> Result<i64> {
    let cutoff = now_unix() - window_secs;
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM login_attempts \
         WHERE user_id = ?1 AND kind = ?2 AND success = 0 AND attempted_at > ?3",
        params![user_id, kind.as_str(), cutoff],
        |row| row.get(0),
    )?;
    Ok(count)
}

/// Return the timestamp of the **oldest** failed attempt for `(user_id, kind)`
/// within the last `window_secs` seconds, or `None` if there are no failures.
///
/// Used to compute the `Retry-After` hint: `oldest + window - now`.
pub fn oldest_failure_ts_in_window(
    conn: &rusqlite::Connection,
    user_id: &str,
    kind: AttemptKind,
    window_secs: i64,
) -> Result<Option<i64>> {
    let cutoff = now_unix() - window_secs;
    let ts: Option<i64> = conn
        .query_row(
            "SELECT MIN(attempted_at) FROM login_attempts \
             WHERE user_id = ?1 AND kind = ?2 AND success = 0 AND attempted_at > ?3",
            params![user_id, kind.as_str(), cutoff],
            |row| row.get::<_, Option<i64>>(0),
        )
        .unwrap_or(None);
    Ok(ts)
}

/// Delete every login_attempts row for `user_id` (both kinds).
///
/// Called by the admin "unlock" endpoint and after a successful login so the
/// counter resets cleanly rather than aging out via the window.
pub fn clear_attempts(conn: &rusqlite::Connection, user_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM login_attempts WHERE user_id = ?1",
        params![user_id],
    )?;
    Ok(())
}

/// Delete every **failed** login_attempts row for `(user_id, kind)`.
///
/// Used after a successful login so the persistent counter resets even though
/// `record_attempt(success=true)` has already inserted its success row.
pub fn clear_failed_attempts(
    conn: &rusqlite::Connection,
    user_id: &str,
    kind: AttemptKind,
) -> Result<()> {
    conn.execute(
        "DELETE FROM login_attempts WHERE user_id = ?1 AND kind = ?2 AND success = 0",
        params![user_id, kind.as_str()],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::UserRole;
    use crate::db::schema;
    use crate::db::users::create_user;

    fn test_conn_with_user() -> (rusqlite::Connection, String) {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        schema::run_migrations(&conn).unwrap();
        let u = create_user(&conn, "alice", UserRole::User).unwrap();
        (conn, u.id)
    }

    #[test]
    fn record_and_count_failures() {
        let (conn, uid) = test_conn_with_user();

        for _ in 0..3 {
            record_attempt(&conn, &uid, AttemptKind::Password, false).unwrap();
        }
        let count =
            failed_count_in_window(&conn, &uid, AttemptKind::Password, 600).unwrap();
        assert_eq!(count, 3);
    }

    #[test]
    fn success_attempts_do_not_count_as_failures() {
        let (conn, uid) = test_conn_with_user();

        record_attempt(&conn, &uid, AttemptKind::Password, false).unwrap();
        record_attempt(&conn, &uid, AttemptKind::Password, true).unwrap();
        record_attempt(&conn, &uid, AttemptKind::Password, false).unwrap();

        let count =
            failed_count_in_window(&conn, &uid, AttemptKind::Password, 600).unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn kind_filter_is_respected() {
        let (conn, uid) = test_conn_with_user();

        record_attempt(&conn, &uid, AttemptKind::Password, false).unwrap();
        record_attempt(&conn, &uid, AttemptKind::Password, false).unwrap();
        record_attempt(&conn, &uid, AttemptKind::Recovery, false).unwrap();

        assert_eq!(
            failed_count_in_window(&conn, &uid, AttemptKind::Password, 600).unwrap(),
            2
        );
        assert_eq!(
            failed_count_in_window(&conn, &uid, AttemptKind::Recovery, 600).unwrap(),
            1
        );
    }

    #[test]
    fn window_filter_excludes_old_attempts() {
        let (conn, uid) = test_conn_with_user();

        // Manually insert an old failure ~20 minutes ago.
        let now = now_unix();
        conn.execute(
            "INSERT INTO login_attempts (user_id, kind, attempted_at, success) VALUES (?1, 'password', ?2, 0)",
            params![uid, now - 1200],
        )
        .unwrap();
        // And a recent one.
        record_attempt(&conn, &uid, AttemptKind::Password, false).unwrap();

        // Only the recent one is within a 10-minute window.
        let count =
            failed_count_in_window(&conn, &uid, AttemptKind::Password, 600).unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn clear_attempts_drops_all_rows() {
        let (conn, uid) = test_conn_with_user();

        record_attempt(&conn, &uid, AttemptKind::Password, false).unwrap();
        record_attempt(&conn, &uid, AttemptKind::Recovery, false).unwrap();
        clear_attempts(&conn, &uid).unwrap();

        assert_eq!(
            failed_count_in_window(&conn, &uid, AttemptKind::Password, 600).unwrap(),
            0
        );
        assert_eq!(
            failed_count_in_window(&conn, &uid, AttemptKind::Recovery, 600).unwrap(),
            0
        );
    }

    #[test]
    fn clear_failed_attempts_keeps_success_rows() {
        let (conn, uid) = test_conn_with_user();

        record_attempt(&conn, &uid, AttemptKind::Password, false).unwrap();
        record_attempt(&conn, &uid, AttemptKind::Password, true).unwrap();
        clear_failed_attempts(&conn, &uid, AttemptKind::Password).unwrap();

        // Failed counter is cleared.
        assert_eq!(
            failed_count_in_window(&conn, &uid, AttemptKind::Password, 600).unwrap(),
            0
        );
        // But the success row is still in the table.
        let total: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM login_attempts WHERE user_id = ?1",
                params![uid],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(total, 1);
    }

    #[test]
    fn oldest_failure_ts_in_window_returns_min_ts() {
        let (conn, uid) = test_conn_with_user();
        let now = now_unix();
        // Two failures within window (200 s ago and 50 s ago) plus a success.
        conn.execute(
            "INSERT INTO login_attempts (user_id, kind, attempted_at, success) VALUES (?1, 'password', ?2, 0)",
            params![uid, now - 200],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO login_attempts (user_id, kind, attempted_at, success) VALUES (?1, 'password', ?2, 0)",
            params![uid, now - 50],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO login_attempts (user_id, kind, attempted_at, success) VALUES (?1, 'password', ?2, 1)",
            params![uid, now - 10],
        )
        .unwrap();

        let oldest = oldest_failure_ts_in_window(&conn, &uid, AttemptKind::Password, 600)
            .unwrap()
            .expect("expected a Some(oldest_ts)");
        assert_eq!(oldest, now - 200);

        // Empty case → None.
        clear_attempts(&conn, &uid).unwrap();
        assert!(
            oldest_failure_ts_in_window(&conn, &uid, AttemptKind::Password, 600)
                .unwrap()
                .is_none()
        );
    }
}
