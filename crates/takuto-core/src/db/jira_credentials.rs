// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! `user_jira_credentials` table — row shape + CRUD.
//!
//! Mirrors [`crate::db::github_credentials`]. The Jira API **token** is the
//! secret and is sealed with the envelope scheme (the four BLOB columns map
//! to [`crate::auth::SealedBlob`]). The Jira **site** base URL and account
//! **email** are stored as plain metadata columns — together they form the
//! Basic-auth pair (`email:token`) used against the Jira REST API. The
//! `account_id` / `account_name` captured at validation time are stored so
//! the dashboard can show "connected as <name>" without another round-trip.
//!
//! ### Adapter contract
//!
//! Writes (`upsert`, `delete`, `touch_last_validated`) take
//! `&mut DbTransaction<'_>` because `routes/credentials.rs` co-commits them
//! with `credential_audit::log_in_tx`. The lone read (`find`) takes
//! `&DbAdapter` since it is called from many non-transactional sites
//! (onboarding status, the Jira read-path resolver).

use crate::auth::SealedBlob;
use crate::db::{DbAdapter, DbTransaction, DbValue};
use crate::error::Result;

/// One row in `user_jira_credentials`. The token is sealed; the four BLOB
/// columns mirror `auth::seal::SealedBlob`.
#[derive(Debug, Clone)]
pub struct JiraCredentialRow {
    pub user_id: String,
    pub ciphertext: Vec<u8>,
    pub nonce: [u8; 24],
    pub wrapped_dek: Vec<u8>,
    pub wnonce: [u8; 24],
    /// Jira site base URL, e.g. `https://acme.atlassian.net` (no trailing slash).
    pub site: String,
    /// Account email used as the Basic-auth username.
    pub email: String,
    /// Atlassian `accountId` captured at validation time.
    pub account_id: String,
    /// Display name captured at validation time.
    pub account_name: String,
    pub last_validated_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

fn now_iso() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

const SELECT_COLS: &str = "user_id, ciphertext, nonce, wrapped_dek, wnonce, site, email, \
     account_id, account_name, last_validated_at, created_at, updated_at";

fn decode_row(r: &crate::db::DbRow) -> Result<JiraCredentialRow> {
    let nonce_blob = r.get_bytes(2)?;
    let wnonce_blob = r.get_bytes(4)?;
    let mut nonce = [0u8; 24];
    let mut wnonce = [0u8; 24];
    if nonce_blob.len() != 24 || wnonce_blob.len() != 24 {
        return Err(crate::db::DbError::NulByte {
            field: "jira_credentials.nonce_or_wnonce_wrong_length",
        }
        .into());
    }
    nonce.copy_from_slice(&nonce_blob);
    wnonce.copy_from_slice(&wnonce_blob);
    Ok(JiraCredentialRow {
        user_id: r.get_text(0)?,
        ciphertext: r.get_bytes(1)?,
        nonce,
        wrapped_dek: r.get_bytes(3)?,
        wnonce,
        site: r.get_text(5)?,
        email: r.get_text(6)?,
        account_id: r.get_text(7)?,
        account_name: r.get_text(8)?,
        last_validated_at: r.get_text_opt(9)?,
        created_at: r.get_text(10)?,
        updated_at: r.get_text(11)?,
    })
}

/// Insert-or-update the user's Jira credential row. Idempotent on `user_id`
/// (the column is a `PRIMARY KEY`). The caller bumps `last_validated_at` via
/// [`touch_last_validated`] after a successful validation.
///
/// Inside a `DbTransaction` so the audit row co-commits.
#[allow(clippy::too_many_arguments)]
pub async fn upsert(
    tx: &mut DbTransaction<'_>,
    user_id: &str,
    sealed: &SealedBlob,
    site: &str,
    email: &str,
    account_id: &str,
    account_name: &str,
) -> Result<()> {
    let now = now_iso();
    let tail = super::upsert::build_update_tail(
        tx.backend(),
        &["user_id"],
        &[
            "ciphertext",
            "nonce",
            "wrapped_dek",
            "wnonce",
            "site",
            "email",
            "account_id",
            "account_name",
            "updated_at",
        ],
    );
    let sql = format!(
        "INSERT INTO user_jira_credentials \
         (user_id, ciphertext, nonce, wrapped_dek, wnonce, site, email, account_id, \
          account_name, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) {tail}"
    );
    tx.execute(
        &sql,
        vec![
            DbValue::Text(user_id.to_string()),
            DbValue::Bytes(sealed.ciphertext.clone()),
            DbValue::Bytes(sealed.nonce.to_vec()),
            DbValue::Bytes(sealed.wrapped_dek.clone()),
            DbValue::Bytes(sealed.wnonce.to_vec()),
            DbValue::Text(site.to_string()),
            DbValue::Text(email.to_string()),
            DbValue::Text(account_id.to_string()),
            DbValue::Text(account_name.to_string()),
            DbValue::Text(now.clone()),
            DbValue::Text(now),
        ],
    )
    .await?;
    Ok(())
}

/// Look up the user's Jira credential row, if any. Read-only; takes
/// `&DbAdapter`.
pub async fn find(adapter: &DbAdapter, user_id: &str) -> Result<Option<JiraCredentialRow>> {
    let sql = format!("SELECT {SELECT_COLS} FROM user_jira_credentials WHERE user_id = ?");
    let row = adapter
        .query_optional(&sql, vec![DbValue::Text(user_id.to_string())])
        .await?;
    row.map(|r| decode_row(&r)).transpose()
}

/// Hard-delete the user's row. Returns `true` when a row existed.
/// Inside a `DbTransaction` so the audit row co-commits.
pub async fn delete(tx: &mut DbTransaction<'_>, user_id: &str) -> Result<bool> {
    let n = tx
        .execute(
            "DELETE FROM user_jira_credentials WHERE user_id = ?",
            vec![DbValue::Text(user_id.to_string())],
        )
        .await?;
    Ok(n > 0)
}

/// Bump `last_validated_at` after a successful credential re-check. Inside a
/// `DbTransaction` because the credential-write path uses it.
pub async fn touch_last_validated(
    tx: &mut DbTransaction<'_>,
    user_id: &str,
    when_iso8601_utc: &str,
) -> Result<()> {
    tx.execute(
        "UPDATE user_jira_credentials SET last_validated_at = ? WHERE user_id = ?",
        vec![
            DbValue::Text(when_iso8601_utc.to_string()),
            DbValue::Text(user_id.to_string()),
        ],
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::MasterKey;
    use crate::auth::seal;
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

    fn sealed_blob(mk: &MasterKey, plaintext: &[u8]) -> SealedBlob {
        seal(mk, plaintext).unwrap()
    }

    #[allow(clippy::too_many_arguments)]
    async fn upsert_committed(
        adapter: &DbAdapter,
        user_id: &str,
        sealed: &SealedBlob,
        site: &str,
        email: &str,
        account_id: &str,
        account_name: &str,
    ) {
        let mut tx = adapter.begin().await.unwrap();
        upsert(
            &mut tx,
            user_id,
            sealed,
            site,
            email,
            account_id,
            account_name,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }

    #[tokio::test]
    async fn upsert_inserts_then_updates_and_token_round_trips() {
        let a = fresh_adapter().await;
        let mk = MasterKey::from_bytes([0xAA; 32]);
        let s1 = sealed_blob(&mk, b"token-v1");
        upsert_committed(
            &a,
            "u-alice",
            &s1,
            "https://acme.atlassian.net",
            "alice@acme.com",
            "acc-1",
            "Alice",
        )
        .await;
        let row = find(&a, "u-alice").await.unwrap().unwrap();
        assert_eq!(row.site, "https://acme.atlassian.net");
        assert_eq!(row.email, "alice@acme.com");
        assert_eq!(row.account_id, "acc-1");
        assert_eq!(row.account_name, "Alice");
        assert!(row.last_validated_at.is_none());

        // Rotate to a new token + email.
        let s2 = sealed_blob(&mk, b"token-v2");
        upsert_committed(
            &a,
            "u-alice",
            &s2,
            "https://acme.atlassian.net",
            "alice2@acme.com",
            "acc-1",
            "Alice A",
        )
        .await;
        let row2 = find(&a, "u-alice").await.unwrap().unwrap();
        assert_eq!(row2.email, "alice2@acme.com");
        assert_eq!(row2.account_name, "Alice A");
        let opened = seal::open(
            &mk,
            &SealedBlob {
                ciphertext: row2.ciphertext.clone(),
                nonce: row2.nonce,
                wrapped_dek: row2.wrapped_dek.clone(),
                wnonce: row2.wnonce,
            },
        )
        .unwrap();
        assert_eq!(opened, b"token-v2");
    }

    #[tokio::test]
    async fn touch_last_validated_sets_timestamp() {
        let a = fresh_adapter().await;
        let mk = MasterKey::from_bytes([0xBB; 32]);
        upsert_committed(
            &a,
            "u-alice",
            &sealed_blob(&mk, b"x"),
            "https://x.atlassian.net",
            "a@x.com",
            "acc",
            "A",
        )
        .await;
        let mut tx = a.begin().await.unwrap();
        touch_last_validated(&mut tx, "u-alice", "2026-06-15T00:00:00Z")
            .await
            .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(
            find(&a, "u-alice")
                .await
                .unwrap()
                .unwrap()
                .last_validated_at
                .as_deref(),
            Some("2026-06-15T00:00:00Z")
        );
    }

    #[tokio::test]
    async fn delete_is_idempotent() {
        let a = fresh_adapter().await;
        let mk = MasterKey::from_bytes([0xCC; 32]);
        upsert_committed(
            &a,
            "u-alice",
            &sealed_blob(&mk, b"x"),
            "https://x.atlassian.net",
            "a@x.com",
            "acc",
            "A",
        )
        .await;
        let mut tx = a.begin().await.unwrap();
        assert!(delete(&mut tx, "u-alice").await.unwrap());
        tx.commit().await.unwrap();

        let mut tx = a.begin().await.unwrap();
        assert!(!delete(&mut tx, "u-alice").await.unwrap());
        tx.commit().await.unwrap();
    }
}
