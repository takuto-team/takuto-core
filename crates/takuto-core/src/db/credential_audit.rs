// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! `credential_audit` table — row shape + CRUD.
//!
//! Insert + list helpers consumed by the credential endpoints.
//!
//! NOT to be confused with the general-purpose `audit_events` table — this
//! one is per-credential, that one is per-user-action.
//!
//! ### Atomicity entry points
//!
//! Two entry points exist for `log`:
//!
//! * [`log_in_tx`] — for `routes/credentials.rs`'s atomic write paths.
//!   Takes `&mut DbTransaction<'_>` so the audit row co-commits with
//!   the upsert/delete in the same transaction.
//! * [`log`] — for the auth-resolver and validator paths that emit
//!   audit rows outside any larger transaction. Takes `&DbAdapter`.

use serde::{Deserialize, Serialize};

use crate::db::{DbAdapter, DbTransaction, DbValue};
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

/// SQL shared by [`log`] and [`log_in_tx`]. The legacy form used
/// SQLite's `strftime` default for the `at` column; we now bind a Rust-
/// computed ISO-8601 string so the SQL works on every backend.
const INSERT_SQL: &str = "INSERT INTO credential_audit \
     (user_id, actor_user_id, kind, provider, event, outcome, error_code, at) \
     VALUES (?, ?, ?, ?, ?, ?, ?, ?)";

fn now_iso() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn bind_log_params(
    user_id: &str,
    actor_user_id: Option<&str>,
    kind: CredentialAuditKind,
    provider: Option<&str>,
    event: &str,
    outcome: &str,
    error_code: Option<&str>,
) -> Vec<DbValue> {
    vec![
        DbValue::Text(user_id.to_string()),
        DbValue::TextOpt(actor_user_id.map(|s| s.to_string())),
        DbValue::Text(kind.as_str().to_string()),
        DbValue::TextOpt(provider.map(|s| s.to_string())),
        DbValue::Text(event.to_string()),
        DbValue::Text(outcome.to_string()),
        DbValue::TextOpt(error_code.map(|s| s.to_string())),
        DbValue::Text(now_iso()),
    ]
}

/// Append a single audit row outside of any explicit transaction. Used by
/// the auth-resolver's audit/validator helpers (one-shot logging with no
/// other DB write to co-commit).
///
/// Failures bubble up as `TakutoError::Db` — callers that want fire-and-
/// forget behaviour discard the result with `let _ = ...`.
#[allow(clippy::too_many_arguments)]
pub async fn log(
    adapter: &DbAdapter,
    user_id: &str,
    actor_user_id: Option<&str>,
    kind: CredentialAuditKind,
    provider: Option<&str>,
    event: &str,
    outcome: &str,
    error_code: Option<&str>,
) -> Result<()> {
    let params = bind_log_params(
        user_id,
        actor_user_id,
        kind,
        provider,
        event,
        outcome,
        error_code,
    );
    adapter.execute(INSERT_SQL, params).await?;
    Ok(())
}

/// Append a single audit row INSIDE an existing transaction. Used by
/// `routes/credentials.rs` so the credential write (provider_credentials
/// or github_credentials) and the audit row commit atomically.
///
/// The route opens the transaction, calls one or more write helpers
/// (with `_in_tx` suffix), then commits or rolls back. A busted audit
/// table fails the whole transaction closed — that's the documented
/// behaviour and the reason this is split from [`log`].
#[allow(clippy::too_many_arguments)]
pub async fn log_in_tx(
    tx: &mut DbTransaction<'_>,
    user_id: &str,
    actor_user_id: Option<&str>,
    kind: CredentialAuditKind,
    provider: Option<&str>,
    event: &str,
    outcome: &str,
    error_code: Option<&str>,
) -> Result<()> {
    let params = bind_log_params(
        user_id,
        actor_user_id,
        kind,
        provider,
        event,
        outcome,
        error_code,
    );
    tx.execute(INSERT_SQL, params).await?;
    Ok(())
}

/// Pull recent audit rows for a single user, newest first. Used by the
/// admin-only audit reader. `limit` is clamped to `[1, 1000]` so a
/// curious caller can't OOM the process.
pub async fn list_for_user(
    adapter: &DbAdapter,
    user_id: &str,
    limit: i64,
) -> Result<Vec<CredentialAuditRow>> {
    let clamped = limit.clamp(1, 1000);
    let rows = adapter
        .query_all(
            "SELECT id, user_id, actor_user_id, kind, provider, event, outcome, error_code, at \
             FROM credential_audit \
             WHERE user_id = ? \
             ORDER BY at DESC, id DESC \
             LIMIT ?",
            vec![DbValue::Text(user_id.to_string()), DbValue::I64(clamped)],
        )
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        let kind_str = r.get_text(3)?;
        let kind = match kind_str.as_str() {
            "ai_provider" => CredentialAuditKind::AiProvider,
            "github_pat" => CredentialAuditKind::GithubPat,
            "cursor_session" => CredentialAuditKind::CursorSession,
            other => {
                // Unknown `kind` is a schema-drift / DB-corruption signal.
                // Match the soft-fail pattern from
                // `user_worktree_commands::get_run_commands_for_pairs`:
                // log + omit so the admin audit view doesn't fail closed
                // on a single bad row.
                tracing::warn!(
                    user_id = %user_id,
                    kind = %other,
                    "skipping credential_audit row: unknown kind"
                );
                continue;
            }
        };
        out.push(CredentialAuditRow {
            id: r.get_i64(0)?,
            user_id: r.get_text(1)?,
            actor_user_id: r.get_text_opt(2)?,
            kind,
            provider: r.get_text_opt(4)?,
            event: r.get_text(5)?,
            outcome: r.get_text(6)?,
            error_code: r.get_text_opt(7)?,
            at: r.get_text(8)?,
        });
    }
    Ok(out)
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
        let adapter = DbAdapter::new(DbPool::Sqlite(pool));
        adapter
            .execute(
                "INSERT INTO users (id, username, role) VALUES ('u-alice', 'alice', 'user')",
                vec![],
            )
            .await
            .unwrap();
        adapter
    }

    #[tokio::test]
    async fn log_then_list_round_trip_returns_newest_first() {
        let a = fresh_adapter().await;
        log(
            &a,
            "u-alice",
            Some("u-alice"),
            CredentialAuditKind::AiProvider,
            Some("claude"),
            "created",
            "ok",
            None,
        )
        .await
        .unwrap();
        // ISO timestamps tick at 1s granularity; sleep so list ordering is
        // deterministic on the (at DESC, id DESC) tie-break.
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        log(
            &a,
            "u-alice",
            Some("u-alice"),
            CredentialAuditKind::GithubPat,
            None,
            "validation_failed",
            "error",
            Some("invalid_pat"),
        )
        .await
        .unwrap();

        let rows = list_for_user(&a, "u-alice", 10).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].event, "validation_failed");
        assert_eq!(rows[0].error_code.as_deref(), Some("invalid_pat"));
        assert!(matches!(rows[0].kind, CredentialAuditKind::GithubPat));
        assert_eq!(rows[1].event, "created");
        assert_eq!(rows[1].provider.as_deref(), Some("claude"));
    }

    #[tokio::test]
    async fn list_for_user_clamps_limit() {
        let a = fresh_adapter().await;
        for _ in 0..3 {
            log(
                &a,
                "u-alice",
                None,
                CredentialAuditKind::AiProvider,
                Some("claude"),
                "created",
                "ok",
                None,
            )
            .await
            .unwrap();
        }
        // limit=0 → clamped to 1.
        assert_eq!(list_for_user(&a, "u-alice", 0).await.unwrap().len(), 1);
        // limit=5000 → clamped to 1000 (we only have 3).
        assert_eq!(list_for_user(&a, "u-alice", 5000).await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn list_for_user_filters_by_user() {
        let a = fresh_adapter().await;
        a.execute(
            "INSERT INTO users (id, username, role) VALUES ('u-bob', 'bob', 'user')",
            vec![],
        )
        .await
        .unwrap();
        log(
            &a,
            "u-alice",
            None,
            CredentialAuditKind::AiProvider,
            Some("claude"),
            "created",
            "ok",
            None,
        )
        .await
        .unwrap();
        log(
            &a,
            "u-bob",
            None,
            CredentialAuditKind::AiProvider,
            Some("cursor"),
            "created",
            "ok",
            None,
        )
        .await
        .unwrap();
        assert_eq!(list_for_user(&a, "u-alice", 10).await.unwrap().len(), 1);
        assert_eq!(list_for_user(&a, "u-bob", 10).await.unwrap().len(), 1);
    }

    /// log_in_tx commits with the surrounding transaction; rollback discards.
    #[tokio::test]
    async fn log_in_tx_co_commits_with_transaction() {
        let a = fresh_adapter().await;
        let mut tx = a.begin().await.unwrap();
        log_in_tx(
            &mut tx,
            "u-alice",
            None,
            CredentialAuditKind::AiProvider,
            Some("claude"),
            "created",
            "ok",
            None,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();

        assert_eq!(list_for_user(&a, "u-alice", 10).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn log_in_tx_rollback_discards_the_row() {
        let a = fresh_adapter().await;
        let mut tx = a.begin().await.unwrap();
        log_in_tx(
            &mut tx,
            "u-alice",
            None,
            CredentialAuditKind::AiProvider,
            Some("claude"),
            "created",
            "ok",
            None,
        )
        .await
        .unwrap();
        tx.rollback().await.unwrap();

        assert!(list_for_user(&a, "u-alice", 10).await.unwrap().is_empty());
    }
}
