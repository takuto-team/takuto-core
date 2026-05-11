// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Password hashing (argon2), passkey storage, and recovery code management.

use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use rusqlite::params;
use uuid::Uuid;

use crate::error::{MaestroError, Result};

/// Hash a password using argon2id with a random salt.
pub fn hash_password(password: &str) -> Result<Vec<u8>> {
    let mut salt_bytes = [0u8; 16];
    getrandom::fill(&mut salt_bytes)
        .map_err(|e| MaestroError::Auth(format!("Failed to generate salt: {e}")))?;
    let salt = SaltString::encode_b64(&salt_bytes)
        .map_err(|e| MaestroError::Auth(format!("Failed to encode salt: {e}")))?;
    let argon2 = Argon2::default();
    let hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| MaestroError::Auth(format!("Failed to hash password: {e}")))?;
    Ok(hash.to_string().into_bytes())
}

/// Verify a password against a stored argon2 hash.
pub fn verify_password(password: &str, stored_hash: &[u8]) -> Result<bool> {
    let hash_str = std::str::from_utf8(stored_hash)
        .map_err(|e| MaestroError::Auth(format!("Invalid stored hash encoding: {e}")))?;
    let parsed_hash = PasswordHash::new(hash_str)
        .map_err(|e| MaestroError::Auth(format!("Invalid password hash format: {e}")))?;
    let argon2 = Argon2::default();
    Ok(argon2
        .verify_password(password.as_bytes(), &parsed_hash)
        .is_ok())
}

/// Store a password credential for a user. Hashes the password before storing.
pub fn store_password(
    conn: &rusqlite::Connection,
    user_id: &str,
    password: &str,
) -> Result<String> {
    let hash = hash_password(password)?;
    let cred_id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO credentials (id, user_id, kind, data, label) VALUES (?1, ?2, 'password', ?3, NULL)",
        params![cred_id, user_id, hash],
    )?;
    Ok(cred_id)
}

/// Verify a password for a user. Returns `true` if any stored password credential matches.
/// Also updates `last_used_at` on successful verification.
pub fn verify_user_password(
    conn: &rusqlite::Connection,
    user_id: &str,
    password: &str,
) -> Result<bool> {
    let mut stmt =
        conn.prepare("SELECT id, data FROM credentials WHERE user_id = ?1 AND kind = 'password'")?;
    let creds: Vec<(String, Vec<u8>)> = stmt
        .query_map(params![user_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?
        .filter_map(|r| r.ok())
        .collect();

    for (cred_id, hash) in creds {
        if verify_password(password, &hash)? {
            // Update last_used_at
            let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
            conn.execute(
                "UPDATE credentials SET last_used_at = ?1 WHERE id = ?2",
                params![now, cred_id],
            )?;
            return Ok(true);
        }
    }
    Ok(false)
}

/// Generate a set of recovery codes and store their hashes. Returns the plaintext codes
/// (to be displayed to the user once).
pub fn generate_recovery_codes(
    conn: &rusqlite::Connection,
    user_id: &str,
    count: usize,
) -> Result<Vec<String>> {
    // Delete any existing recovery codes for this user.
    conn.execute(
        "DELETE FROM recovery_codes WHERE user_id = ?1",
        params![user_id],
    )?;

    let mut codes = Vec::with_capacity(count);
    for _ in 0..count {
        let code = generate_code();
        let hash = hash_password(&code)?;
        let id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO recovery_codes (id, user_id, code_hash, used) VALUES (?1, ?2, ?3, 0)",
            params![id, user_id, hash],
        )?;
        codes.push(code);
    }
    Ok(codes)
}

/// Verify a recovery code and mark it as used (single-use). Returns `true` if valid and unused.
pub fn verify_and_consume_recovery_code(
    conn: &rusqlite::Connection,
    user_id: &str,
    code: &str,
) -> Result<bool> {
    let mut stmt =
        conn.prepare("SELECT id, code_hash FROM recovery_codes WHERE user_id = ?1 AND used = 0")?;
    let codes: Vec<(String, Vec<u8>)> = stmt
        .query_map(params![user_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?
        .filter_map(|r| r.ok())
        .collect();

    for (code_id, hash) in codes {
        if verify_password(code, &hash)? {
            conn.execute(
                "UPDATE recovery_codes SET used = 1 WHERE id = ?1",
                params![code_id],
            )?;
            return Ok(true);
        }
    }
    Ok(false)
}

/// Delete all sessions for a user (for use on suspend/password change).
pub fn delete_user_sessions(conn: &rusqlite::Connection, user_id: &str) -> Result<()> {
    conn.execute("DELETE FROM sessions WHERE user_id = ?1", params![user_id])?;
    Ok(())
}

/// Generate a random recovery code in the format `XXXX-XXXX` (8 alphanumeric chars with a dash).
fn generate_code() -> String {
    use rand::Rng;
    let mut rng = rand::rng();
    let charset: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789"; // Excluded confusable chars: 0OI1
    let part = |rng: &mut rand::rngs::ThreadRng| -> String {
        (0..4)
            .map(|_| charset[rng.random_range(0..charset.len())] as char)
            .collect()
    };
    format!("{}-{}", part(&mut rng), part(&mut rng))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema;

    fn test_conn_with_user() -> (rusqlite::Connection, String) {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        schema::run_migrations(&conn).unwrap();
        let user = crate::db::users::create_user(&conn, "alice", crate::db::models::UserRole::User)
            .unwrap();
        (conn, user.id)
    }

    #[test]
    fn password_hash_and_verify() {
        let hash = hash_password("secret123").unwrap();
        assert!(verify_password("secret123", &hash).unwrap());
        assert!(!verify_password("wrong", &hash).unwrap());
    }

    #[test]
    fn store_and_verify_password() {
        let (conn, user_id) = test_conn_with_user();
        store_password(&conn, &user_id, "mypassword").unwrap();
        assert!(verify_user_password(&conn, &user_id, "mypassword").unwrap());
        assert!(!verify_user_password(&conn, &user_id, "wrong").unwrap());
    }

    #[test]
    fn recovery_codes_generated_and_verified() {
        let (conn, user_id) = test_conn_with_user();
        let codes = generate_recovery_codes(&conn, &user_id, 8).unwrap();
        assert_eq!(codes.len(), 8);

        // Each code should be verifiable once.
        let first = &codes[0];
        assert!(verify_and_consume_recovery_code(&conn, &user_id, first).unwrap());
        // Second use should fail (already consumed).
        assert!(!verify_and_consume_recovery_code(&conn, &user_id, first).unwrap());
    }

    #[test]
    fn recovery_code_wrong_code_rejected() {
        let (conn, user_id) = test_conn_with_user();
        let _ = generate_recovery_codes(&conn, &user_id, 4).unwrap();
        assert!(!verify_and_consume_recovery_code(&conn, &user_id, "XXXX-YYYY").unwrap());
    }

    #[test]
    fn regenerate_recovery_codes_replaces_old() {
        let (conn, user_id) = test_conn_with_user();
        let codes1 = generate_recovery_codes(&conn, &user_id, 4).unwrap();
        let codes2 = generate_recovery_codes(&conn, &user_id, 4).unwrap();

        // Old codes should no longer work.
        assert!(!verify_and_consume_recovery_code(&conn, &user_id, &codes1[0]).unwrap());
        // New codes should work.
        assert!(verify_and_consume_recovery_code(&conn, &user_id, &codes2[0]).unwrap());
    }

    #[test]
    fn delete_user_sessions_works() {
        let (conn, user_id) = test_conn_with_user();
        // Insert a fake session.
        conn.execute(
            "INSERT INTO sessions (id, user_id, data, expires_at) VALUES ('s1', ?1, X'00', '2099-01-01T00:00:00Z')",
            params![user_id],
        )
        .unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE user_id = ?1",
                params![user_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        delete_user_sessions(&conn, &user_id).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE user_id = ?1",
                params![user_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn generated_code_format() {
        let code = generate_code();
        assert_eq!(code.len(), 9); // XXXX-XXXX
        assert_eq!(code.chars().nth(4), Some('-'));
    }
}
