// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.
//! `user_provider_credentials` table — row shape + CRUD.
//!
//! Insert / select / mark-inactive helpers consumed by the per-user
//! credential endpoints in `crates/maestro-web/src/routes/credentials.rs`.
//!
//! ### API shapes
//!
//! * **Reads** (`find_active`, `find_active_with_kind`,
//!   `find_all_for_user`) take `&DbAdapter` — called from many
//!   non-transactional sites (auth resolver, status route, bundle
//!   builder).
//! * **Writes** (`upsert`, `delete`, `delete_with_kind`) take
//!   `&mut DbTransaction<'_>` — `routes/credentials.rs` co-commits
//!   them with the audit row in one transaction, preserving the
//!   atomicity invariant. The three helpers that aren't called from
//!   routes (`mark_inactive`, `touch_last_validated`, `touch_last_used`)
//!   take `&DbAdapter` since no transactional caller exists.

use serde::{Deserialize, Serialize};

use crate::auth::SealedBlob;
use crate::db::{DbAdapter, DbTransaction, DbValue};
use crate::error::{MaestroError, Result};

/// `kind` discriminator. v1 only writes `ApiKey`; the other two are reserved
/// for Claude OAuth and a potential future Cursor CLI-state path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCredentialKind {
    ApiKey,
    OauthToken,
    CliState,
}

impl ProviderCredentialKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderCredentialKind::ApiKey => "api_key",
            ProviderCredentialKind::OauthToken => "oauth_token",
            ProviderCredentialKind::CliState => "cli_state",
        }
    }
}

/// One row in `user_provider_credentials`. All sealed fields are opaque blobs;
/// the plaintext is recovered via `auth::seal::open` with the deployment
/// master key.
#[derive(Debug, Clone)]
pub struct ProviderCredentialRow {
    pub id: i64,
    pub user_id: String,
    /// `"claude" | "cursor" | "codex" | "opencode"`. Stored as a string for
    /// forward compatibility (`AiAgentProvider` already enumerates the
    /// values).
    pub provider: String,
    pub kind: ProviderCredentialKind,
    /// AEAD-sealed plaintext (api key, oauth bearer, etc.).
    pub ciphertext: Vec<u8>,
    pub nonce: [u8; 24],
    /// DEK sealed with the master key.
    pub wrapped_dek: Vec<u8>,
    pub wnonce: [u8; 24],
    /// Free-form, NON-secret metadata: account label, validated scopes, etc.
    pub metadata_json: String,
    /// `true` after a deployment-wide provider switch (kept for audit).
    pub inactive: bool,
    pub last_validated_at: Option<String>,
    pub last_used_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub expires_at: Option<String>,
}

/// Returned by [`upsert`] so the credential-save handler can branch on
/// "first time we've stored this provider for the user" vs "rotation".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpsertOutcome {
    Created,
    Rotated,
}

impl UpsertOutcome {
    /// Stable audit-log event string.
    pub fn audit_event(self) -> &'static str {
        match self {
            UpsertOutcome::Created => "created",
            UpsertOutcome::Rotated => "rotated",
        }
    }
}

/// Application-computed timestamp matching the legacy
/// `strftime('%Y-%m-%dT%H:%M:%SZ','now')` shape. Bound explicitly so the
/// SQL works on every backend (SQLite, Postgres, MySQL).
fn now_iso() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Insert a fresh credential row OR update the existing
/// `(user_id, provider, kind)` row with new sealed bytes + metadata.
///
/// Returns `Created` for a brand-new row, `Rotated` for an in-place
/// rotation. The row is always marked `inactive = 0` on write (rotating
/// a credential implicitly reactivates a previously-inactivated row).
///
/// Takes `&mut DbTransaction` because the credential write co-commits
/// with the audit row in `routes/credentials.rs`. The SELECT-then-
/// UPDATE-or-INSERT race window the legacy implementation tolerated is
/// preserved because both ops run inside the same transaction.
pub async fn upsert(
    tx: &mut DbTransaction<'_>,
    user_id: &str,
    provider: &str,
    kind: ProviderCredentialKind,
    sealed: &SealedBlob,
    metadata_json: &str,
) -> Result<UpsertOutcome> {
    let existing = tx
        .query_optional(
            "SELECT id FROM user_provider_credentials \
             WHERE user_id = ? AND provider = ? AND kind = ?",
            vec![
                DbValue::Text(user_id.to_string()),
                DbValue::Text(provider.to_string()),
                DbValue::Text(kind.as_str().to_string()),
            ],
        )
        .await?;

    match existing {
        Some(row) => {
            let id = row.get_i64(0)?;
            tx.execute(
                "UPDATE user_provider_credentials \
                 SET ciphertext = ?, nonce = ?, wrapped_dek = ?, wnonce = ?, \
                     metadata_json = ?, inactive = 0, updated_at = ? \
                 WHERE id = ?",
                vec![
                    DbValue::Bytes(sealed.ciphertext.clone()),
                    DbValue::Bytes(sealed.nonce.to_vec()),
                    DbValue::Bytes(sealed.wrapped_dek.clone()),
                    DbValue::Bytes(sealed.wnonce.to_vec()),
                    DbValue::Text(metadata_json.to_string()),
                    DbValue::Text(now_iso()),
                    DbValue::I64(id),
                ],
            )
            .await?;
            Ok(UpsertOutcome::Rotated)
        }
        None => {
            let now = now_iso();
            tx.execute(
                "INSERT INTO user_provider_credentials \
                 (user_id, provider, kind, ciphertext, nonce, wrapped_dek, wnonce, \
                  metadata_json, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                vec![
                    DbValue::Text(user_id.to_string()),
                    DbValue::Text(provider.to_string()),
                    DbValue::Text(kind.as_str().to_string()),
                    DbValue::Bytes(sealed.ciphertext.clone()),
                    DbValue::Bytes(sealed.nonce.to_vec()),
                    DbValue::Bytes(sealed.wrapped_dek.clone()),
                    DbValue::Bytes(sealed.wnonce.to_vec()),
                    DbValue::Text(metadata_json.to_string()),
                    DbValue::Text(now.clone()),
                    DbValue::Text(now),
                ],
            )
            .await?;
            Ok(UpsertOutcome::Created)
        }
    }
}

/// Project a SELECT row into a `ProviderCredentialRow`. Used by every
/// read helper. Column order matches `SELECT_COLS` (defined inline in
/// each query for clarity).
fn decode_row(r: &crate::db::DbRow) -> Result<ProviderCredentialRow> {
    let nonce_blob = r.get_bytes(5)?;
    let wnonce_blob = r.get_bytes(7)?;
    let mut nonce = [0u8; 24];
    let mut wnonce = [0u8; 24];
    if nonce_blob.len() != 24 || wnonce_blob.len() != 24 {
        // Schema invariant: nonce / wnonce are always 24 bytes. A short
        // value is corruption. Propagate as a typed DbError.
        return Err(crate::db::DbError::NulByte {
            field: "provider_credentials.nonce_or_wnonce_wrong_length",
        }
        .into());
    }
    nonce.copy_from_slice(&nonce_blob);
    wnonce.copy_from_slice(&wnonce_blob);

    let kind_str = r.get_text(3)?;
    let kind = match kind_str.as_str() {
        "api_key" => ProviderCredentialKind::ApiKey,
        "oauth_token" => ProviderCredentialKind::OauthToken,
        "cli_state" => ProviderCredentialKind::CliState,
        _ => {
            return Err(crate::db::DbError::NulByte {
                field: "provider_credentials.kind_unknown",
            }
            .into());
        }
    };
    Ok(ProviderCredentialRow {
        id: r.get_i64(0)?,
        user_id: r.get_text(1)?,
        provider: r.get_text(2)?,
        kind,
        ciphertext: r.get_bytes(4)?,
        nonce,
        wrapped_dek: r.get_bytes(6)?,
        wnonce,
        metadata_json: r.get_text(8)?,
        inactive: r.get_i64(9)? != 0,
        last_validated_at: r.get_text_opt(10)?,
        last_used_at: r.get_text_opt(11)?,
        created_at: r.get_text(12)?,
        updated_at: r.get_text(13)?,
        expires_at: r.get_text_opt(14)?,
    })
}

/// Common column projection — must match `decode_row`'s positional layout.
const SELECT_COLS: &str = "id, user_id, provider, kind, ciphertext, nonce, wrapped_dek, wnonce, \
     metadata_json, inactive, last_validated_at, last_used_at, \
     created_at, updated_at, expires_at";

/// Return ONE active (`inactive = 0`) row for `(user_id, provider)`.
/// Claude users can have both `api_key` and `cli_state` rows
/// simultaneously, so the lookup is deterministic: prefer `api_key` first,
/// then `cli_state`, then `oauth_token`. Callers that only need a
/// presence probe ("does this user have ANY credential for this
/// provider?") keep the same behaviour. Callers that need a specific kind
/// use [`find_active_with_kind`].
pub async fn find_active(
    adapter: &DbAdapter,
    user_id: &str,
    provider: &str,
) -> Result<Option<ProviderCredentialRow>> {
    let sql = format!(
        "SELECT {SELECT_COLS} \
         FROM user_provider_credentials \
         WHERE user_id = ? AND provider = ? AND inactive = 0 \
         ORDER BY CASE kind \
                    WHEN 'api_key'     THEN 0 \
                    WHEN 'cli_state'   THEN 1 \
                    WHEN 'oauth_token' THEN 2 \
                    ELSE 3 \
                  END \
         LIMIT 1"
    );
    let row = adapter
        .query_optional(
            &sql,
            vec![
                DbValue::Text(user_id.to_string()),
                DbValue::Text(provider.to_string()),
            ],
        )
        .await?;
    row.map(|r| decode_row(&r)).transpose()
}

/// Return the single active row for `(user_id, provider, kind)` (or
/// `None`). This is what the worker bundle uses to assemble the per-kind
/// tmpfs files separately — one Claude user might have api_key only,
/// cli_state only, or both, and the bundle builder needs to query each
/// independently.
pub async fn find_active_with_kind(
    adapter: &DbAdapter,
    user_id: &str,
    provider: &str,
    kind: ProviderCredentialKind,
) -> Result<Option<ProviderCredentialRow>> {
    let sql = format!(
        "SELECT {SELECT_COLS} \
         FROM user_provider_credentials \
         WHERE user_id = ? AND provider = ? AND kind = ? AND inactive = 0 \
         LIMIT 1"
    );
    let row = adapter
        .query_optional(
            &sql,
            vec![
                DbValue::Text(user_id.to_string()),
                DbValue::Text(provider.to_string()),
                DbValue::Text(kind.as_str().to_string()),
            ],
        )
        .await?;
    row.map(|r| decode_row(&r)).transpose()
}

/// Return every row for `user_id` regardless of `inactive` — drives
/// `GET /api/users/me/credentials`.
pub async fn find_all_for_user(
    adapter: &DbAdapter,
    user_id: &str,
) -> Result<Vec<ProviderCredentialRow>> {
    let sql = format!(
        "SELECT {SELECT_COLS} \
         FROM user_provider_credentials \
         WHERE user_id = ? \
         ORDER BY provider, kind"
    );
    let rows = adapter
        .query_all(&sql, vec![DbValue::Text(user_id.to_string())])
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        out.push(decode_row(r)?);
    }
    Ok(out)
}

/// Hard-delete every row for `(user_id, provider)`. Idempotent; returns
/// `true` when at least one row was deleted (the audit handler uses this to
/// skip the audit emit on a no-op delete).
///
/// Inside a `DbTransaction` so the audit row co-commits.
pub async fn delete(
    tx: &mut DbTransaction<'_>,
    user_id: &str,
    provider: &str,
) -> Result<bool> {
    let n = tx
        .execute(
            "DELETE FROM user_provider_credentials WHERE user_id = ? AND provider = ?",
            vec![
                DbValue::Text(user_id.to_string()),
                DbValue::Text(provider.to_string()),
            ],
        )
        .await?;
    Ok(n > 0)
}

/// Hard-delete the single row for `(user_id, provider, kind)`, leaving
/// any other-kind rows for the same `(user, provider)` intact.
/// Drives the `DELETE /api/users/me/credentials/{provider}?kind=cli_state`
/// flow so the UI can wipe just the session state without touching the
/// api_key row (or vice versa).
pub async fn delete_with_kind(
    tx: &mut DbTransaction<'_>,
    user_id: &str,
    provider: &str,
    kind: ProviderCredentialKind,
) -> Result<bool> {
    let n = tx
        .execute(
            "DELETE FROM user_provider_credentials \
             WHERE user_id = ? AND provider = ? AND kind = ?",
            vec![
                DbValue::Text(user_id.to_string()),
                DbValue::Text(provider.to_string()),
                DbValue::Text(kind.as_str().to_string()),
            ],
        )
        .await?;
    Ok(n > 0)
}

/// Mark every row for `(user_id, provider)` as `inactive = 1`. Used when
/// the deployment-wide provider switches (the old creds stay for audit /
/// restore — see 04_architecture.md §2.4).
pub async fn mark_inactive(adapter: &DbAdapter, user_id: &str, provider: &str) -> Result<()> {
    adapter
        .execute(
            "UPDATE user_provider_credentials SET inactive = 1, updated_at = ? \
             WHERE user_id = ? AND provider = ?",
            vec![
                DbValue::Text(now_iso()),
                DbValue::Text(user_id.to_string()),
                DbValue::Text(provider.to_string()),
            ],
        )
        .await?;
    Ok(())
}

/// Bump `last_validated_at` to the supplied ISO-8601 UTC timestamp.
pub async fn touch_last_validated(
    adapter: &DbAdapter,
    id: i64,
    when_iso8601_utc: &str,
) -> Result<()> {
    adapter
        .execute(
            "UPDATE user_provider_credentials SET last_validated_at = ? WHERE id = ?",
            vec![
                DbValue::Text(when_iso8601_utc.to_string()),
                DbValue::I64(id),
            ],
        )
        .await?;
    Ok(())
}

/// Bump `last_used_at` to the supplied ISO-8601 UTC timestamp.
pub async fn touch_last_used(adapter: &DbAdapter, id: i64, when_iso8601_utc: &str) -> Result<()> {
    adapter
        .execute(
            "UPDATE user_provider_credentials SET last_used_at = ? WHERE id = ?",
            vec![
                DbValue::Text(when_iso8601_utc.to_string()),
                DbValue::I64(id),
            ],
        )
        .await?;
    Ok(())
}

/// Convenience: typed error for the "no master key, can't seal" path.
pub fn err_master_key_unavailable() -> MaestroError {
    crate::config::ConfigError::MasterKeyUnavailable.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::seal;
    use crate::auth::MasterKey;
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

    /// Helper: run upsert + commit in one closed scope so the call sites
    /// stay readable. The route does this inline alongside an audit::log_in_tx.
    async fn upsert_committed(
        adapter: &DbAdapter,
        user_id: &str,
        provider: &str,
        kind: ProviderCredentialKind,
        sealed: &SealedBlob,
        metadata_json: &str,
    ) -> UpsertOutcome {
        let mut tx = adapter.begin().await.unwrap();
        let outcome = upsert(&mut tx, user_id, provider, kind, sealed, metadata_json)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        outcome
    }

    #[tokio::test]
    async fn upsert_creates_then_rotates() {
        let a = fresh_adapter().await;
        let mk = MasterKey::from_bytes([0xAA; 32]);
        let sealed_a = seal(&mk, b"pat-A").unwrap();
        let sealed_b = seal(&mk, b"pat-B").unwrap();

        let r1 = upsert_committed(
            &a,
            "u-alice",
            "claude",
            ProviderCredentialKind::ApiKey,
            &sealed_a,
            "{}",
        )
        .await;
        assert_eq!(r1, UpsertOutcome::Created);

        let r2 = upsert_committed(
            &a,
            "u-alice",
            "claude",
            ProviderCredentialKind::ApiKey,
            &sealed_b,
            "{}",
        )
        .await;
        assert_eq!(r2, UpsertOutcome::Rotated);

        // Only one row exists for the (user, provider, kind) tuple.
        let row = a
            .query_one(
                "SELECT COUNT(*) FROM user_provider_credentials WHERE user_id = 'u-alice'",
                vec![],
            )
            .await
            .unwrap();
        assert_eq!(row.get_i64(0).unwrap(), 1);
    }

    #[tokio::test]
    async fn find_active_round_trips_through_seal_open() {
        let a = fresh_adapter().await;
        let mk = MasterKey::from_bytes([0xCC; 32]);
        let sealed = seal(&mk, b"super-secret-token").unwrap();
        upsert_committed(
            &a,
            "u-alice",
            "cursor",
            ProviderCredentialKind::ApiKey,
            &sealed,
            "{}",
        )
        .await;

        let row = find_active(&a, "u-alice", "cursor").await.unwrap().unwrap();
        let opened = seal::open(
            &mk,
            &SealedBlob {
                ciphertext: row.ciphertext.clone(),
                nonce: row.nonce,
                wrapped_dek: row.wrapped_dek.clone(),
                wnonce: row.wnonce,
            },
        )
        .unwrap();
        assert_eq!(opened, b"super-secret-token");
    }

    #[tokio::test]
    async fn find_active_skips_inactive_rows() {
        let a = fresh_adapter().await;
        let mk = MasterKey::from_bytes([0x55; 32]);
        let sealed = seal(&mk, b"x").unwrap();
        upsert_committed(
            &a,
            "u-alice",
            "claude",
            ProviderCredentialKind::ApiKey,
            &sealed,
            "{}",
        )
        .await;
        mark_inactive(&a, "u-alice", "claude").await.unwrap();

        assert!(find_active(&a, "u-alice", "claude").await.unwrap().is_none());
        // But find_all still sees it.
        assert_eq!(find_all_for_user(&a, "u-alice").await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn delete_removes_row_and_returns_true_only_on_hit() {
        let a = fresh_adapter().await;
        let mk = MasterKey::from_bytes([0x77; 32]);
        let sealed = seal(&mk, b"x").unwrap();
        upsert_committed(
            &a,
            "u-alice",
            "claude",
            ProviderCredentialKind::ApiKey,
            &sealed,
            "{}",
        )
        .await;

        let mut tx = a.begin().await.unwrap();
        assert!(delete(&mut tx, "u-alice", "claude").await.unwrap());
        tx.commit().await.unwrap();

        let mut tx = a.begin().await.unwrap();
        assert!(!delete(&mut tx, "u-alice", "claude").await.unwrap());
        tx.commit().await.unwrap();

        assert!(find_active(&a, "u-alice", "claude").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn touch_helpers_set_timestamps_on_id() {
        let a = fresh_adapter().await;
        let mk = MasterKey::from_bytes([0x11; 32]);
        let sealed = seal(&mk, b"x").unwrap();
        upsert_committed(
            &a,
            "u-alice",
            "claude",
            ProviderCredentialKind::ApiKey,
            &sealed,
            "{}",
        )
        .await;

        let row = find_active(&a, "u-alice", "claude").await.unwrap().unwrap();
        assert!(row.last_validated_at.is_none());
        touch_last_validated(&a, row.id, "2026-05-18T01:00:00Z")
            .await
            .unwrap();
        touch_last_used(&a, row.id, "2026-05-18T02:00:00Z")
            .await
            .unwrap();

        let row2 = find_active(&a, "u-alice", "claude").await.unwrap().unwrap();
        assert_eq!(row2.last_validated_at.as_deref(), Some("2026-05-18T01:00:00Z"));
        assert_eq!(row2.last_used_at.as_deref(), Some("2026-05-18T02:00:00Z"));
    }
}
