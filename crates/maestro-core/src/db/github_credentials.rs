// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! `user_github_credentials` table — row shape + CRUD.
//!
//! Phase 2a defined the shape; Phase 2b.1 grows the helpers consumed by the
//! `POST /api/users/me/github-pat` endpoint.
//!
//! **Wire-vs-column rename** — the JSON wire field is `attribute_commits`
//! (per arch doc A3 — clarifies the v1 toggle is git author/committer
//! attribution, not GPG/SSH signing), but the SQLite column is
//! `sign_commits` (held over from a pre-A3 draft; renaming would require a
//! migration we don't need yet). The HTTP layer uses
//! `#[serde(rename = "attribute_commits")]` at the request-body boundary so
//! the column name stays internal.

use rusqlite::{Connection, OptionalExtension, params};

use crate::auth::SealedBlob;
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

fn row_from_query(row: &rusqlite::Row) -> rusqlite::Result<GitHubCredentialRow> {
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
    Ok(GitHubCredentialRow {
        user_id: row.get("user_id")?,
        ciphertext: row.get("ciphertext")?,
        nonce,
        wrapped_dek: row.get("wrapped_dek")?,
        wnonce,
        github_login: row.get("github_login")?,
        scopes_json: row.get("scopes_json")?,
        sign_commits: row.get::<_, i64>("sign_commits")? != 0,
        last_validated_at: row.get("last_validated_at")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

/// Insert-or-update the user's GitHub credential row. Idempotent on
/// `user_id` (the column is a `PRIMARY KEY`). Resets `last_validated_at` to
/// NULL on update — the caller bumps it via [`touch_last_validated`] after a
/// successful validation.
pub fn upsert(
    conn: &Connection,
    user_id: &str,
    sealed: &SealedBlob,
    github_login: &str,
    scopes_json: &str,
    sign_commits: bool,
) -> Result<()> {
    conn.execute(
        "INSERT INTO user_github_credentials \
         (user_id, ciphertext, nonce, wrapped_dek, wnonce, github_login, scopes_json, sign_commits) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
         ON CONFLICT(user_id) DO UPDATE SET \
            ciphertext = excluded.ciphertext, \
            nonce = excluded.nonce, \
            wrapped_dek = excluded.wrapped_dek, \
            wnonce = excluded.wnonce, \
            github_login = excluded.github_login, \
            scopes_json = excluded.scopes_json, \
            sign_commits = excluded.sign_commits, \
            updated_at = strftime('%Y-%m-%dT%H:%M:%SZ','now')",
        params![
            user_id,
            sealed.ciphertext.as_slice(),
            sealed.nonce.as_slice(),
            sealed.wrapped_dek.as_slice(),
            sealed.wnonce.as_slice(),
            github_login,
            scopes_json,
            i64::from(sign_commits),
        ],
    )?;
    Ok(())
}

/// Look up the user's GitHub credential row, if any.
pub fn find(conn: &Connection, user_id: &str) -> Result<Option<GitHubCredentialRow>> {
    let mut stmt = conn.prepare(
        "SELECT user_id, ciphertext, nonce, wrapped_dek, wnonce, github_login, scopes_json, \
                sign_commits, last_validated_at, created_at, updated_at \
         FROM user_github_credentials WHERE user_id = ?1",
    )?;
    let row = stmt
        .query_row(params![user_id], row_from_query)
        .optional()?;
    Ok(row)
}

/// Hard-delete the user's row. Returns `true` when a row existed.
pub fn delete(conn: &Connection, user_id: &str) -> Result<bool> {
    let n = conn.execute(
        "DELETE FROM user_github_credentials WHERE user_id = ?1",
        params![user_id],
    )?;
    Ok(n > 0)
}

/// Update the A3 commit-attribution toggle. Returns `true` when the row
/// existed and was updated, `false` when there was no row to update (the
/// PATCH handler treats this as a 404 caller-side).
pub fn set_sign_commits(conn: &Connection, user_id: &str, value: bool) -> Result<bool> {
    let n = conn.execute(
        "UPDATE user_github_credentials SET sign_commits = ?1, \
         updated_at = strftime('%Y-%m-%dT%H:%M:%SZ','now') \
         WHERE user_id = ?2",
        params![i64::from(value), user_id],
    )?;
    Ok(n > 0)
}

/// Bump `last_validated_at` after a successful PAT re-check.
pub fn touch_last_validated(
    conn: &Connection,
    user_id: &str,
    when_iso8601_utc: &str,
) -> Result<()> {
    conn.execute(
        "UPDATE user_github_credentials SET last_validated_at = ?1 WHERE user_id = ?2",
        params![when_iso8601_utc, user_id],
    )?;
    Ok(())
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
        conn.execute(
            "INSERT INTO users (id, username, role) VALUES ('u-alice', 'alice', 'user')",
            [],
        )
        .unwrap();
        conn
    }

    fn sealed_blob(mk: &MasterKey, plaintext: &[u8]) -> SealedBlob {
        seal(mk, plaintext).unwrap()
    }

    #[test]
    fn upsert_inserts_then_updates_same_user() {
        let conn = fresh_db();
        let mk = MasterKey::from_bytes([0xAA; 32]);
        let s1 = sealed_blob(&mk, b"pat-v1");
        upsert(&conn, "u-alice", &s1, "alice-gh", "[\"repo\"]", true).unwrap();
        let row = find(&conn, "u-alice").unwrap().unwrap();
        assert_eq!(row.github_login, "alice-gh");
        assert!(row.sign_commits);

        // Rotate.
        let s2 = sealed_blob(&mk, b"pat-v2");
        upsert(
            &conn,
            "u-alice",
            &s2,
            "alice-gh",
            "[\"repo\",\"read:org\"]",
            false,
        )
        .unwrap();
        let row2 = find(&conn, "u-alice").unwrap().unwrap();
        assert_eq!(row2.scopes_json, "[\"repo\",\"read:org\"]");
        assert!(!row2.sign_commits);
        // Round-trip via seal/open to confirm the sealed bytes really got rewritten.
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

    #[test]
    fn set_sign_commits_returns_false_when_no_row() {
        let conn = fresh_db();
        assert!(!set_sign_commits(&conn, "u-alice", false).unwrap());
    }

    #[test]
    fn set_sign_commits_flips_value() {
        let conn = fresh_db();
        let mk = MasterKey::from_bytes([0xBB; 32]);
        upsert(
            &conn,
            "u-alice",
            &sealed_blob(&mk, b"x"),
            "alice",
            "[]",
            true,
        )
        .unwrap();
        assert!(set_sign_commits(&conn, "u-alice", false).unwrap());
        assert!(!find(&conn, "u-alice").unwrap().unwrap().sign_commits);
    }

    #[test]
    fn delete_is_idempotent() {
        let conn = fresh_db();
        let mk = MasterKey::from_bytes([0xCC; 32]);
        upsert(
            &conn,
            "u-alice",
            &sealed_blob(&mk, b"x"),
            "alice",
            "[]",
            true,
        )
        .unwrap();
        assert!(delete(&conn, "u-alice").unwrap());
        assert!(!delete(&conn, "u-alice").unwrap());
    }
}
