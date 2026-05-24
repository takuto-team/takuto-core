// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! `credential_audit` table — row shape + CRUD.
//!
//! Phase 2a defined the table; Phase 2b.1 grows the insert + list helpers
//! consumed by the credential endpoints.
//!
//! NOT to be confused with the general-purpose `audit_events` table reserved
//! for plan-03 (different team, different concern: this one is per-credential,
//! that one is per-user-action).

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::error::Result;

/// Discriminator for the credential the row refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialAuditKind {
    AiProvider,
    GithubPat,
    /// Reserved variant (carry-over from the original Cursor ttyd-capture
    /// design that amendment A1 cancelled). Never written by current code.
    /// Kept in the enum so any historical rows in pre-A1 databases still
    /// deserialise cleanly; deleting would require a schema migration.
    CursorSession,
}

impl CredentialAuditKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CredentialAuditKind::AiProvider => "ai_provider",
            CredentialAuditKind::GithubPat => "github_pat",
            CredentialAuditKind::CursorSession => "cursor_session",
        }
    }
}

/// One row of credential audit history.
#[derive(Debug, Clone)]
pub struct CredentialAuditRow {
    pub id: i64,
    pub user_id: String,
    /// `None` for system actions (e.g. cascade invalidation on provider
    /// switch); `Some(admin_user_id)` for admin-impersonation paths.
    pub actor_user_id: Option<String>,
    pub kind: CredentialAuditKind,
    /// Provider name when `kind == AiProvider`; `None` for `GithubPat` /
    /// `CursorSession`.
    pub provider: Option<String>,
    /// `"created" | "rotated" | "deleted" | "validation_failed" | "invalidated_provider_switch"`.
    pub event: String,
    /// `"ok" | "error"`.
    pub outcome: String,
    /// Classified error code (never the raw upstream body) when
    /// `outcome == "error"`.
    pub error_code: Option<String>,
    /// ISO-8601 UTC timestamp.
    pub at: String,
}

/// Append a single audit row. Failures bubble up as `MaestroError::Db`
/// (`DbError::Sqlite`) — the credential handler propagates them so a busted
/// audit table fails the operation closed (we want a credential write +
/// audit row to be atomic; the handler runs both in the same transaction).
#[allow(clippy::too_many_arguments)]
pub fn log(
    conn: &Connection,
    user_id: &str,
    actor_user_id: Option<&str>,
    kind: CredentialAuditKind,
    provider: Option<&str>,
    event: &str,
    outcome: &str,
    error_code: Option<&str>,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO credential_audit \
         (user_id, actor_user_id, kind, provider, event, outcome, error_code) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            user_id,
            actor_user_id,
            kind.as_str(),
            provider,
            event,
            outcome,
            error_code,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Pull recent audit rows for a single user, newest first. Used by the
/// admin-only audit reader that lands in Phase 2b.2. `limit` is clamped to
/// `[1, 1000]` so a curious caller can't OOM the process.
pub fn list_for_user(
    conn: &Connection,
    user_id: &str,
    limit: i64,
) -> Result<Vec<CredentialAuditRow>> {
    let clamped = limit.clamp(1, 1000);
    let mut stmt = conn.prepare(
        "SELECT id, user_id, actor_user_id, kind, provider, event, outcome, error_code, at \
         FROM credential_audit \
         WHERE user_id = ?1 \
         ORDER BY at DESC, id DESC \
         LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![user_id, clamped], |row| {
            let kind_str: String = row.get("kind")?;
            let kind = match kind_str.as_str() {
                "ai_provider" => CredentialAuditKind::AiProvider,
                "github_pat" => CredentialAuditKind::GithubPat,
                "cursor_session" => CredentialAuditKind::CursorSession,
                other => {
                    return Err(rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        format!("unknown credential_audit kind {other}").into(),
                    ));
                }
            };
            Ok(CredentialAuditRow {
                id: row.get("id")?,
                user_id: row.get("user_id")?,
                actor_user_id: row.get("actor_user_id")?,
                kind,
                provider: row.get("provider")?,
                event: row.get("event")?,
                outcome: row.get("outcome")?,
                error_code: row.get("error_code")?,
                at: row.get("at")?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema;

    fn fresh_db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        schema::run_migrations(&conn).unwrap();
        conn.execute(
            "INSERT INTO users (id, username, role) VALUES ('u-alice', 'alice', 'user')",
            [],
        )
        .unwrap();
        conn
    }

    #[test]
    fn log_then_list_round_trip_returns_newest_first() {
        let conn = fresh_db();
        log(
            &conn,
            "u-alice",
            Some("u-alice"),
            CredentialAuditKind::AiProvider,
            Some("claude"),
            "created",
            "ok",
            None,
        )
        .unwrap();
        // ISO timestamps tick at 1s granularity; sleep so list ordering is
        // deterministic on the (at DESC, id DESC) tie-break.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        log(
            &conn,
            "u-alice",
            Some("u-alice"),
            CredentialAuditKind::GithubPat,
            None,
            "validation_failed",
            "error",
            Some("invalid_pat"),
        )
        .unwrap();

        let rows = list_for_user(&conn, "u-alice", 10).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].event, "validation_failed");
        assert_eq!(rows[0].error_code.as_deref(), Some("invalid_pat"));
        assert!(matches!(rows[0].kind, CredentialAuditKind::GithubPat));
        assert_eq!(rows[1].event, "created");
        assert_eq!(rows[1].provider.as_deref(), Some("claude"));
    }

    #[test]
    fn list_for_user_clamps_limit() {
        let conn = fresh_db();
        for _ in 0..3 {
            log(
                &conn,
                "u-alice",
                None,
                CredentialAuditKind::AiProvider,
                Some("claude"),
                "created",
                "ok",
                None,
            )
            .unwrap();
        }
        // limit=0 → clamped to 1.
        assert_eq!(list_for_user(&conn, "u-alice", 0).unwrap().len(), 1);
        // limit=5000 → clamped to 1000 (we only have 3).
        assert_eq!(list_for_user(&conn, "u-alice", 5000).unwrap().len(), 3);
    }

    #[test]
    fn list_for_user_filters_by_user() {
        let conn = fresh_db();
        conn.execute(
            "INSERT INTO users (id, username, role) VALUES ('u-bob', 'bob', 'user')",
            [],
        )
        .unwrap();
        log(
            &conn,
            "u-alice",
            None,
            CredentialAuditKind::AiProvider,
            Some("claude"),
            "created",
            "ok",
            None,
        )
        .unwrap();
        log(
            &conn,
            "u-bob",
            None,
            CredentialAuditKind::AiProvider,
            Some("cursor"),
            "created",
            "ok",
            None,
        )
        .unwrap();
        assert_eq!(list_for_user(&conn, "u-alice", 10).unwrap().len(), 1);
        assert_eq!(list_for_user(&conn, "u-bob", 10).unwrap().len(), 1);
    }
}
