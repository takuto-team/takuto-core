// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! `user_github_credentials` table — row shape + CRUD.
//!
//! Helpers consumed by the `POST /api/users/me/github-pat` endpoint.
//!
//! **Wire-vs-column rename** — the JSON wire field is `attribute_commits`
//! (per arch doc A3 — clarifies the v1 toggle is git author/committer
//! attribution, not GPG/SSH signing), but the SQLite column is
//! `sign_commits` (held over from a pre-A3 draft; renaming would require a
//! migration we don't need yet). The HTTP layer uses
//! `#[serde(rename = "attribute_commits")]` at the request-body boundary so
//! the column name stays internal.
//!
//! ### Adapter contract
//!
//! Writes (`upsert`, `delete`, `set_sign_commits`, `touch_last_validated`)
//! take `&mut DbTransaction<'_>` because `routes/credentials.rs`
//! co-commits them with `credential_audit::log_in_tx`. The lone read
//! (`find`) takes `&DbAdapter` since it's called from many
//! non-transactional sites.

use crate::auth::SealedBlob;
use crate::db::{DbAdapter, DbTransaction, DbValue};
use crate::error::Result;

/// One row in `user_github_credentials`. PAT is sealed via the envelope
/// scheme; the four BLOB columns mirror `auth::seal::SealedBlob`.
#[derive(Debug, Clone)]
pub struct GitHubCredentialRow {
    pub user_id: String,
    pub ciphertext: Vec<u8>,
    pub nonce: [u8; 24],
    pub wrapped_dek: Vec<u8>,
    pub wnonce: [u8; 24],
    /// GitHub login captured at save time (e.g. `morphet81`).
    pub github_login: String,
    /// JSON array of scopes the PAT was validated against
    /// (e.g. `["repo","read:org"]`).
    pub scopes_json: String,
    /// Per A3: this controls git author/committer attribution
    /// (NOT GPG/SSH cryptographic signing). Column name retained for
    /// stability; the UI label is "Attribute commits to me".
    pub sign_commits: bool,
    pub last_validated_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

fn now_iso() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

const SELECT_COLS: &str = "user_id, ciphertext, nonce, wrapped_dek, wnonce, github_login, scopes_json, \
     sign_commits, last_validated_at, created_at, updated_at";

fn decode_row(r: &crate::db::DbRow) -> Result<GitHubCredentialRow> {
    let nonce_blob = r.get_bytes(2)?;
    let wnonce_blob = r.get_bytes(4)?;
    let mut nonce = [0u8; 24];
    let mut wnonce = [0u8; 24];
    if nonce_blob.len() != 24 || wnonce_blob.len() != 24 {
        return Err(crate::db::DbError::NulByte {
            field: "github_credentials.nonce_or_wnonce_wrong_length",
        }
        .into());
    }
    nonce.copy_from_slice(&nonce_blob);
    wnonce.copy_from_slice(&wnonce_blob);
    Ok(GitHubCredentialRow {
        user_id: r.get_text(0)?,
        ciphertext: r.get_bytes(1)?,
        nonce,
        wrapped_dek: r.get_bytes(3)?,
        wnonce,
        github_login: r.get_text(5)?,
        scopes_json: r.get_text(6)?,
        sign_commits: r.get_i64(7)? != 0,
        last_validated_at: r.get_text_opt(8)?,
        created_at: r.get_text(9)?,
        updated_at: r.get_text(10)?,
    })
}

/// Insert-or-update the user's GitHub credential row. Idempotent on
/// `user_id` (the column is a `PRIMARY KEY`). Resets `last_validated_at` to
/// NULL on update — the caller bumps it via [`touch_last_validated`] after a
/// successful validation.
///
/// Inside a `DbTransaction` so the audit row co-commits.
pub async fn upsert(
    tx: &mut DbTransaction<'_>,
    user_id: &str,
    sealed: &SealedBlob,
    github_login: &str,
    scopes_json: &str,
    sign_commits: bool,
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
            "github_login",
            "scopes_json",
            "sign_commits",
            "updated_at",
        ],
    );
    let sql = format!(
        "INSERT INTO user_github_credentials \
         (user_id, ciphertext, nonce, wrapped_dek, wnonce, github_login, scopes_json, \
          sign_commits, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) {tail}"
    );
    tx.execute(
        &sql,
        vec![
            DbValue::Text(user_id.to_string()),
            DbValue::Bytes(sealed.ciphertext.clone()),
            DbValue::Bytes(sealed.nonce.to_vec()),
            DbValue::Bytes(sealed.wrapped_dek.clone()),
            DbValue::Bytes(sealed.wnonce.to_vec()),
            DbValue::Text(github_login.to_string()),
            DbValue::Text(scopes_json.to_string()),
            DbValue::I64(i64::from(sign_commits)),
            DbValue::Text(now.clone()),
            DbValue::Text(now),
        ],
    )
    .await?;
    Ok(())
}

/// Look up the user's GitHub credential row, if any. Read-only; takes
/// `&DbAdapter`.
pub async fn find(adapter: &DbAdapter, user_id: &str) -> Result<Option<GitHubCredentialRow>> {
    let sql = format!("SELECT {SELECT_COLS} FROM user_github_credentials WHERE user_id = ?");
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
            "DELETE FROM user_github_credentials WHERE user_id = ?",
            vec![DbValue::Text(user_id.to_string())],
        )
        .await?;
    Ok(n > 0)
}

/// Update the A3 commit-attribution toggle. Returns `true` when the row
/// existed and was updated, `false` when there was no row to update (the
/// PATCH handler treats this as a 404 caller-side).
///
/// Inside a `DbTransaction` so the audit row co-commits.
pub async fn set_sign_commits(
    tx: &mut DbTransaction<'_>,
    user_id: &str,
    value: bool,
) -> Result<bool> {
    let n = tx
        .execute(
            "UPDATE user_github_credentials SET sign_commits = ?, updated_at = ? \
             WHERE user_id = ?",
            vec![
                DbValue::I64(i64::from(value)),
                DbValue::Text(now_iso()),
                DbValue::Text(user_id.to_string()),
            ],
        )
        .await?;
    Ok(n > 0)
}

/// Bump `last_validated_at` after a successful PAT re-check. Inside a
/// `DbTransaction` because the credential-write path uses it; the
/// auth-resolver's debounced "first use" touch lives in a separate path
/// (see `github/auth_resolver/audit.rs`) — that one calls
/// [`touch_last_validated_adapter`] which begins+commits its own short
/// transaction.
pub async fn touch_last_validated(
    tx: &mut DbTransaction<'_>,
    user_id: &str,
    when_iso8601_utc: &str,
) -> Result<()> {
    tx.execute(
        "UPDATE user_github_credentials SET last_validated_at = ? WHERE user_id = ?",
        vec![
            DbValue::Text(when_iso8601_utc.to_string()),
            DbValue::Text(user_id.to_string()),
        ],
    )
    .await?;
    Ok(())
}

/// Non-transactional variant of [`touch_last_validated`] for the auth
/// resolver's debounced "first use within ~60 s" bump. Opens a short
/// transaction internally; one-shot writes don't need to co-commit with
/// anything else.
pub async fn touch_last_validated_adapter(
    adapter: &DbAdapter,
    user_id: &str,
    when_iso8601_utc: &str,
) -> Result<()> {
    adapter
        .execute(
            "UPDATE user_github_credentials SET last_validated_at = ? WHERE user_id = ?",
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

    /// Helper: upsert + commit. Routes do this alongside an audit::log_in_tx;
    /// tests just want the row persisted.
    async fn upsert_committed(
        adapter: &DbAdapter,
        user_id: &str,
        sealed: &SealedBlob,
        github_login: &str,
        scopes_json: &str,
        sign_commits: bool,
    ) {
        let mut tx = adapter.begin().await.unwrap();
        upsert(
            &mut tx,
            user_id,
            sealed,
            github_login,
            scopes_json,
            sign_commits,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }

    #[tokio::test]
    async fn upsert_inserts_then_updates_same_user() {
        let a = fresh_adapter().await;
        let mk = MasterKey::from_bytes([0xAA; 32]);
        let s1 = sealed_blob(&mk, b"pat-v1");
        upsert_committed(&a, "u-alice", &s1, "alice-gh", "[\"repo\"]", true).await;
        let row = find(&a, "u-alice").await.unwrap().unwrap();
        assert_eq!(row.github_login, "alice-gh");
        assert!(row.sign_commits);

        // Rotate.
        let s2 = sealed_blob(&mk, b"pat-v2");
        upsert_committed(
            &a,
            "u-alice",
            &s2,
            "alice-gh",
            "[\"repo\",\"read:org\"]",
            false,
        )
        .await;
        let row2 = find(&a, "u-alice").await.unwrap().unwrap();
        assert_eq!(row2.scopes_json, "[\"repo\",\"read:org\"]");
        assert!(!row2.sign_commits);
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
        assert_eq!(opened, b"pat-v2");
    }

    #[tokio::test]
    async fn set_sign_commits_returns_false_when_no_row() {
        let a = fresh_adapter().await;
        let mut tx = a.begin().await.unwrap();
        assert!(!set_sign_commits(&mut tx, "u-alice", false).await.unwrap());
        tx.commit().await.unwrap();
    }

    #[tokio::test]
    async fn set_sign_commits_flips_value() {
        let a = fresh_adapter().await;
        let mk = MasterKey::from_bytes([0xBB; 32]);
        upsert_committed(&a, "u-alice", &sealed_blob(&mk, b"x"), "alice", "[]", true).await;

        let mut tx = a.begin().await.unwrap();
        assert!(set_sign_commits(&mut tx, "u-alice", false).await.unwrap());
        tx.commit().await.unwrap();

        assert!(!find(&a, "u-alice").await.unwrap().unwrap().sign_commits);
    }

    #[tokio::test]
    async fn delete_is_idempotent() {
        let a = fresh_adapter().await;
        let mk = MasterKey::from_bytes([0xCC; 32]);
        upsert_committed(&a, "u-alice", &sealed_blob(&mk, b"x"), "alice", "[]", true).await;

        let mut tx = a.begin().await.unwrap();
        assert!(delete(&mut tx, "u-alice").await.unwrap());
        tx.commit().await.unwrap();

        let mut tx = a.begin().await.unwrap();
        assert!(!delete(&mut tx, "u-alice").await.unwrap());
        tx.commit().await.unwrap();
    }
}
