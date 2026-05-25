// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.
#![allow(deprecated)] // Transitional: ConfigStr sites rewritten to ConfigError variants in C2.

//! `user_provider_credentials` table — row shape + CRUD.
//!
//! Phase 2a defined the table; Phase 2b.1 grows the insert / select /
//! mark-inactive helpers consumed by the per-user credential endpoints in
//! `crates/maestro-web/src/routes/credentials.rs`.
//!
//! All helpers take a `&rusqlite::Connection` so callers can wrap multiple
//! writes in their own transaction (the audit-log row is co-written with the
//! credential write in the credential handler).

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::auth::SealedBlob;
use crate::error::{MaestroError, Result};

/// `kind` discriminator. v1 only writes `ApiKey`; the other two are reserved
/// for Phase 2b (Claude OAuth) and a potential future Cursor CLI-state path.
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
    /// forward compatibility (Phase 4's `AiAgentProvider` already enumerates
    /// the values).
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

/// Insert a fresh credential row OR update the existing
/// `(user_id, provider, kind)` row with new sealed bytes + metadata. Returns
/// `Created` for a brand-new row, `Rotated` for an in-place rotation. The
/// row is always marked `inactive = 0` on write (rotating a credential
/// implicitly reactivates a previously-inactivated row).
pub fn upsert(
    conn: &Connection,
    user_id: &str,
    provider: &str,
    kind: ProviderCredentialKind,
    sealed: &SealedBlob,
    metadata_json: &str,
) -> Result<UpsertOutcome> {
    let existing: Option<i64> = conn
        .query_row(
            "SELECT id FROM user_provider_credentials \
             WHERE user_id = ?1 AND provider = ?2 AND kind = ?3",
            params![user_id, provider, kind.as_str()],
            |r| r.get(0),
        )
        .optional()?;

    match existing {
        Some(id) => {
            conn.execute(
                "UPDATE user_provider_credentials \
                 SET ciphertext = ?1, nonce = ?2, wrapped_dek = ?3, wnonce = ?4, \
                     metadata_json = ?5, inactive = 0, \
                     updated_at = strftime('%Y-%m-%dT%H:%M:%SZ','now') \
                 WHERE id = ?6",
                params![
                    sealed.ciphertext.as_slice(),
                    sealed.nonce.as_slice(),
                    sealed.wrapped_dek.as_slice(),
                    sealed.wnonce.as_slice(),
                    metadata_json,
                    id,
                ],
            )?;
            Ok(UpsertOutcome::Rotated)
        }
        None => {
            conn.execute(
                "INSERT INTO user_provider_credentials \
                 (user_id, provider, kind, ciphertext, nonce, wrapped_dek, wnonce, metadata_json) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    user_id,
                    provider,
                    kind.as_str(),
                    sealed.ciphertext.as_slice(),
                    sealed.nonce.as_slice(),
                    sealed.wrapped_dek.as_slice(),
                    sealed.wnonce.as_slice(),
                    metadata_json,
                ],
            )?;
            Ok(UpsertOutcome::Created)
        }
    }
}

fn row_from_query(row: &rusqlite::Row) -> rusqlite::Result<ProviderCredentialRow> {
    let nonce_blob: Vec<u8> = row.get("nonce")?;
    let wnonce_blob: Vec<u8> = row.get("wnonce")?;
    let mut nonce = [0u8; 24];
    let mut wnonce = [0u8; 24];
    if nonce_blob.len() != 24 || wnonce_blob.len() != 24 {
        return Err(rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Blob,
            format!(
                "nonce/wnonce wrong length: {}/{} (expected 24/24)",
                nonce_blob.len(),
                wnonce_blob.len()
            )
            .into(),
        ));
    }
    nonce.copy_from_slice(&nonce_blob);
    wnonce.copy_from_slice(&wnonce_blob);
    let kind_str: String = row.get("kind")?;
    let kind = match kind_str.as_str() {
        "api_key" => ProviderCredentialKind::ApiKey,
        "oauth_token" => ProviderCredentialKind::OauthToken,
        "cli_state" => ProviderCredentialKind::CliState,
        other => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                format!("unknown provider credential kind {other}").into(),
            ));
        }
    };
    Ok(ProviderCredentialRow {
        id: row.get("id")?,
        user_id: row.get("user_id")?,
        provider: row.get("provider")?,
        kind,
        ciphertext: row.get("ciphertext")?,
        nonce,
        wrapped_dek: row.get("wrapped_dek")?,
        wnonce,
        metadata_json: row.get("metadata_json")?,
        inactive: row.get::<_, i64>("inactive")? != 0,
        last_validated_at: row.get("last_validated_at")?,
        last_used_at: row.get("last_used_at")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
        expires_at: row.get("expires_at")?,
    })
}

/// Return ONE active (`inactive = 0`) row for `(user_id, provider)`. With
/// task #39 Claude users can have both `api_key` and `cli_state` rows
/// simultaneously, so the lookup is deterministic: prefer `api_key` first,
/// then `cli_state`, then `oauth_token`. Existing callers that only need a
/// presence probe ("does this user have ANY credential for this provider?")
/// keep the same behaviour. Callers that need a specific kind use
/// [`find_active_with_kind`].
pub fn find_active(
    conn: &Connection,
    user_id: &str,
    provider: &str,
) -> Result<Option<ProviderCredentialRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, provider, kind, ciphertext, nonce, wrapped_dek, wnonce, \
                metadata_json, inactive, last_validated_at, last_used_at, \
                created_at, updated_at, expires_at \
         FROM user_provider_credentials \
         WHERE user_id = ?1 AND provider = ?2 AND inactive = 0 \
         ORDER BY CASE kind \
                    WHEN 'api_key'     THEN 0 \
                    WHEN 'cli_state'   THEN 1 \
                    WHEN 'oauth_token' THEN 2 \
                    ELSE 3 \
                  END \
         LIMIT 1",
    )?;
    let row = stmt
        .query_row(params![user_id, provider], row_from_query)
        .optional()?;
    Ok(row)
}

/// Task #39: return the single active row for `(user_id, provider, kind)`
/// (or `None`). This is what the worker bundle uses to assemble the
/// per-kind tmpfs files separately — one Claude user might have api_key
/// only, cli_state only, or both, and the bundle builder needs to query
/// each independently.
pub fn find_active_with_kind(
    conn: &Connection,
    user_id: &str,
    provider: &str,
    kind: ProviderCredentialKind,
) -> Result<Option<ProviderCredentialRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, provider, kind, ciphertext, nonce, wrapped_dek, wnonce, \
                metadata_json, inactive, last_validated_at, last_used_at, \
                created_at, updated_at, expires_at \
         FROM user_provider_credentials \
         WHERE user_id = ?1 AND provider = ?2 AND kind = ?3 AND inactive = 0 \
         LIMIT 1",
    )?;
    let row = stmt
        .query_row(
            params![user_id, provider, kind.as_str()],
            row_from_query,
        )
        .optional()?;
    Ok(row)
}

/// Return every row for `user_id` regardless of `inactive` — drives
/// `GET /api/users/me/credentials`.
pub fn find_all_for_user(
    conn: &Connection,
    user_id: &str,
) -> Result<Vec<ProviderCredentialRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, provider, kind, ciphertext, nonce, wrapped_dek, wnonce, \
                metadata_json, inactive, last_validated_at, last_used_at, \
                created_at, updated_at, expires_at \
         FROM user_provider_credentials \
         WHERE user_id = ?1 \
         ORDER BY provider, kind",
    )?;
    let rows = stmt
        .query_map(params![user_id], row_from_query)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Hard-delete every row for `(user_id, provider)`. Idempotent; returns
/// `true` when at least one row was deleted (the audit handler uses this to
/// skip the audit emit on a no-op delete).
pub fn delete(conn: &Connection, user_id: &str, provider: &str) -> Result<bool> {
    let n = conn.execute(
        "DELETE FROM user_provider_credentials WHERE user_id = ?1 AND provider = ?2",
        params![user_id, provider],
    )?;
    Ok(n > 0)
}

/// Task #39: hard-delete the single row for `(user_id, provider, kind)`,
/// leaving any other-kind rows for the same `(user, provider)` intact.
/// Drives the `DELETE /api/users/me/credentials/{provider}?kind=cli_state`
/// flow so the UI can wipe just the session state without touching the
/// api_key row (or vice versa).
pub fn delete_with_kind(
    conn: &Connection,
    user_id: &str,
    provider: &str,
    kind: ProviderCredentialKind,
) -> Result<bool> {
    let n = conn.execute(
        "DELETE FROM user_provider_credentials \
         WHERE user_id = ?1 AND provider = ?2 AND kind = ?3",
        params![user_id, provider, kind.as_str()],
    )?;
    Ok(n > 0)
}

/// Mark every row for `(user_id, provider)` as `inactive = 1`. Phase 2b.2
/// uses this when the deployment-wide provider switches (the old creds stay
/// for audit / restore — see 04_architecture.md §2.4).
pub fn mark_inactive(conn: &Connection, user_id: &str, provider: &str) -> Result<()> {
    conn.execute(
        "UPDATE user_provider_credentials SET inactive = 1, \
         updated_at = strftime('%Y-%m-%dT%H:%M:%SZ','now') \
         WHERE user_id = ?1 AND provider = ?2",
        params![user_id, provider],
    )?;
    Ok(())
}

/// Bump `last_validated_at` to the supplied ISO-8601 UTC timestamp.
pub fn touch_last_validated(conn: &Connection, id: i64, when_iso8601_utc: &str) -> Result<()> {
    conn.execute(
        "UPDATE user_provider_credentials SET last_validated_at = ?1 WHERE id = ?2",
        params![when_iso8601_utc, id],
    )?;
    Ok(())
}

/// Bump `last_used_at` to the supplied ISO-8601 UTC timestamp.
pub fn touch_last_used(conn: &Connection, id: i64, when_iso8601_utc: &str) -> Result<()> {
    conn.execute(
        "UPDATE user_provider_credentials SET last_used_at = ?1 WHERE id = ?2",
        params![when_iso8601_utc, id],
    )?;
    Ok(())
}

/// Convenience: typed error for the "no master key, can't seal" path.
pub fn err_master_key_unavailable() -> MaestroError {
    MaestroError::ConfigStr("master_key_unavailable".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::seal;
    use crate::auth::MasterKey;
    use crate::db::schema;

    fn fresh_db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        schema::run_migrations(&conn).unwrap();
        // Seed a user so FKs are happy.
        conn.execute(
            "INSERT INTO users (id, username, role) VALUES ('u-alice', 'alice', 'user')",
            [],
        )
        .unwrap();
        conn
    }

    #[test]
    fn upsert_creates_then_rotates() {
        let conn = fresh_db();
        let mk = MasterKey::from_bytes([0xAA; 32]);
        let sealed_a = seal(&mk, b"pat-A").unwrap();
        let sealed_b = seal(&mk, b"pat-B").unwrap();

        let r1 = upsert(
            &conn,
            "u-alice",
            "claude",
            ProviderCredentialKind::ApiKey,
            &sealed_a,
            "{}",
        )
        .unwrap();
        assert_eq!(r1, UpsertOutcome::Created);

        let r2 = upsert(
            &conn,
            "u-alice",
            "claude",
            ProviderCredentialKind::ApiKey,
            &sealed_b,
            "{}",
        )
        .unwrap();
        assert_eq!(r2, UpsertOutcome::Rotated);

        // Only one row exists for the (user, provider, kind) tuple.
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM user_provider_credentials WHERE user_id = 'u-alice'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn find_active_round_trips_through_seal_open() {
        let conn = fresh_db();
        let mk = MasterKey::from_bytes([0xCC; 32]);
        let sealed = seal(&mk, b"super-secret-token").unwrap();
        upsert(
            &conn,
            "u-alice",
            "cursor",
            ProviderCredentialKind::ApiKey,
            &sealed,
            "{}",
        )
        .unwrap();

        let row = find_active(&conn, "u-alice", "cursor").unwrap().unwrap();
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

    #[test]
    fn find_active_skips_inactive_rows() {
        let conn = fresh_db();
        let mk = MasterKey::from_bytes([0x55; 32]);
        let sealed = seal(&mk, b"x").unwrap();
        upsert(
            &conn,
            "u-alice",
            "claude",
            ProviderCredentialKind::ApiKey,
            &sealed,
            "{}",
        )
        .unwrap();
        mark_inactive(&conn, "u-alice", "claude").unwrap();

        assert!(find_active(&conn, "u-alice", "claude").unwrap().is_none());
        // But find_all still sees it.
        assert_eq!(find_all_for_user(&conn, "u-alice").unwrap().len(), 1);
    }

    #[test]
    fn delete_removes_row_and_returns_true_only_on_hit() {
        let conn = fresh_db();
        let mk = MasterKey::from_bytes([0x77; 32]);
        let sealed = seal(&mk, b"x").unwrap();
        upsert(
            &conn,
            "u-alice",
            "claude",
            ProviderCredentialKind::ApiKey,
            &sealed,
            "{}",
        )
        .unwrap();

        assert!(delete(&conn, "u-alice", "claude").unwrap());
        assert!(!delete(&conn, "u-alice", "claude").unwrap()); // idempotent no-op
        assert!(find_active(&conn, "u-alice", "claude").unwrap().is_none());
    }

    #[test]
    fn touch_helpers_set_timestamps_on_id() {
        let conn = fresh_db();
        let mk = MasterKey::from_bytes([0x11; 32]);
        let sealed = seal(&mk, b"x").unwrap();
        upsert(
            &conn,
            "u-alice",
            "claude",
            ProviderCredentialKind::ApiKey,
            &sealed,
            "{}",
        )
        .unwrap();

        let row = find_active(&conn, "u-alice", "claude").unwrap().unwrap();
        assert!(row.last_validated_at.is_none());
        touch_last_validated(&conn, row.id, "2026-05-18T01:00:00Z").unwrap();
        touch_last_used(&conn, row.id, "2026-05-18T02:00:00Z").unwrap();

        let row2 = find_active(&conn, "u-alice", "claude").unwrap().unwrap();
        assert_eq!(row2.last_validated_at.as_deref(), Some("2026-05-18T01:00:00Z"));
        assert_eq!(row2.last_used_at.as_deref(), Some("2026-05-18T02:00:00Z"));
    }
}
