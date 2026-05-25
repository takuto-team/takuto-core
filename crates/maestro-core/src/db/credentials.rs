// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Password hashing (argon2), passkey storage, and recovery code management.
//!
//! ## Argon2id parameters
//!
//! Passwords are hashed with **Argon2id** using OWASP-recommended parameters:
//! - **Passwords:** `m_cost=47104` (≈ 46 MiB), `t_cost=1`, `p_cost=1`.
//! - **Recovery codes:** `m_cost=47104` (≈ 46 MiB), `t_cost=3`, `p_cost=1` —
//!   higher work factor because recovery codes are high-value, single-use, and
//!   verified far less frequently than passwords.
//!
//! On successful password verification, [`verify_user_password`] checks whether
//! the stored hash uses weaker parameters than the current target via
//! [`is_password_hash_weaker_than_current`] and silently re-hashes the password
//! with [`current_argon2_password`] when it does. This lets us upgrade params
//! over time without forced resets. Rehash failures are logged but never break
//! the login (a successful verify always returns `Ok(true)`).

use argon2::password_hash::SaltString;
use argon2::{Algorithm, Argon2, Params, PasswordHash, PasswordHasher, PasswordVerifier, Version};
use rusqlite::params;
use uuid::Uuid;

use crate::error::{MaestroError, Result};

/// Current Argon2id `m_cost` (KiB) for both passwords and recovery codes.
const CURRENT_M_COST: u32 = 47104;
/// Current Argon2id `t_cost` (iterations) for passwords.
const CURRENT_T_COST_PASSWORD: u32 = 1;
/// Current Argon2id `t_cost` (iterations) for recovery codes — stronger because
/// recovery codes are single-use and high-value.
const CURRENT_T_COST_RECOVERY: u32 = 3;
/// Current Argon2id `p_cost` (parallelism) for both passwords and recovery codes.
const CURRENT_P_COST: u32 = 1;

/// Produce a password hash using `Argon2::default()` parameters
/// (`m=19456`, `t=2`, `p=1` at the time of writing).
///
/// **Test helper only.** Exposed publicly so integration tests in sibling
/// crates can seed a credentials row that mimics a pre-upgrade hash, without
/// needing to depend on the `argon2` crate directly. Do not call from
/// production code paths — new hashes must go through [`hash_password`] so
/// they pick up the current OWASP-recommended parameters.
// Transitional: AuthStr sites rewritten to typed AuthError variants in C2.
#[allow(deprecated)]
pub fn legacy_argon2_default_hash_for_tests(password: &str) -> Result<Vec<u8>> {
    let mut salt_bytes = [0u8; 16];
    getrandom::fill(&mut salt_bytes)
        .map_err(|e| MaestroError::AuthStr(format!("Failed to generate salt: {e}")))?;
    let salt = SaltString::encode_b64(&salt_bytes)
        .map_err(|e| MaestroError::AuthStr(format!("Failed to encode salt: {e}")))?;
    let argon2 = Argon2::default();
    let hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| MaestroError::AuthStr(format!("Failed to hash password: {e}")))?;
    Ok(hash.to_string().into_bytes())
}

/// Argon2id instance with current password-hashing parameters.
///
/// Used by [`hash_password`] for new password hashes. Verification reads the
/// PHC string's embedded params; the instance returned here is only used to
/// drive the verifier (it does **not** override the stored params).
fn current_argon2_password() -> Argon2<'static> {
    let params = Params::new(
        CURRENT_M_COST,
        CURRENT_T_COST_PASSWORD,
        CURRENT_P_COST,
        None,
    )
    .expect("static argon2 password params are valid");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

/// Argon2id instance with current recovery-code-hashing parameters (stronger `t_cost`).
fn current_argon2_recovery() -> Argon2<'static> {
    let params = Params::new(
        CURRENT_M_COST,
        CURRENT_T_COST_RECOVERY,
        CURRENT_P_COST,
        None,
    )
    .expect("static argon2 recovery params are valid");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

/// Hash a password using Argon2id with the current parameters and a random salt.
// Transitional: AuthStr sites rewritten to typed AuthError variants in C2.
#[allow(deprecated)]
pub fn hash_password(password: &str) -> Result<Vec<u8>> {
    let mut salt_bytes = [0u8; 16];
    getrandom::fill(&mut salt_bytes)
        .map_err(|e| MaestroError::AuthStr(format!("Failed to generate salt: {e}")))?;
    let salt = SaltString::encode_b64(&salt_bytes)
        .map_err(|e| MaestroError::AuthStr(format!("Failed to encode salt: {e}")))?;
    let argon2 = current_argon2_password();
    let hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| MaestroError::AuthStr(format!("Failed to hash password: {e}")))?;
    Ok(hash.to_string().into_bytes())
}

/// Hash a recovery code using Argon2id with the stronger recovery-code parameters.
// Transitional: AuthStr sites rewritten to typed AuthError variants in C2.
#[allow(deprecated)]
fn hash_recovery_code(code: &str) -> Result<Vec<u8>> {
    let mut salt_bytes = [0u8; 16];
    getrandom::fill(&mut salt_bytes)
        .map_err(|e| MaestroError::AuthStr(format!("Failed to generate salt: {e}")))?;
    let salt = SaltString::encode_b64(&salt_bytes)
        .map_err(|e| MaestroError::AuthStr(format!("Failed to encode salt: {e}")))?;
    let argon2 = current_argon2_recovery();
    let hash = argon2
        .hash_password(code.as_bytes(), &salt)
        .map_err(|e| MaestroError::AuthStr(format!("Failed to hash recovery code: {e}")))?;
    Ok(hash.to_string().into_bytes())
}

/// Verify a password against a stored Argon2 hash.
///
/// The verifier reads the PHC string's embedded parameters, so this works
/// regardless of which (older or current) parameter set produced `stored_hash`.
// Transitional: AuthStr sites rewritten to typed AuthError variants in C2.
#[allow(deprecated)]
pub fn verify_password(password: &str, stored_hash: &[u8]) -> Result<bool> {
    let hash_str = std::str::from_utf8(stored_hash)
        .map_err(|e| MaestroError::AuthStr(format!("Invalid stored hash encoding: {e}")))?;
    let parsed_hash = PasswordHash::new(hash_str)
        .map_err(|e| MaestroError::AuthStr(format!("Invalid password hash format: {e}")))?;
    let argon2 = current_argon2_password();
    Ok(argon2
        .verify_password(password.as_bytes(), &parsed_hash)
        .is_ok())
}

/// Returns `true` when the stored Argon2 hash's parameters are weaker than the
/// current password-hashing target (`m_cost=47104`, `t_cost=1`, `p_cost=1`).
///
/// Any single dimension being below the target counts as weaker. Malformed
/// hashes return `Err` — callers should log and skip the rehash on error.
// Transitional: AuthStr sites rewritten to typed AuthError variants in C2.
#[allow(deprecated)]
fn is_password_hash_weaker_than_current(stored_hash: &[u8]) -> Result<bool> {
    let hash_str = std::str::from_utf8(stored_hash)
        .map_err(|e| MaestroError::AuthStr(format!("Invalid stored hash encoding: {e}")))?;
    let parsed = PasswordHash::new(hash_str)
        .map_err(|e| MaestroError::AuthStr(format!("Invalid password hash format: {e}")))?;
    // PHC params are typed as `ParamsString`; we read them out via the `Params`
    // conversion which exposes typed accessors.
    let params = Params::try_from(&parsed)
        .map_err(|e| MaestroError::AuthStr(format!("Failed to read argon2 params: {e}")))?;
    Ok(params.m_cost() < CURRENT_M_COST
        || params.t_cost() < CURRENT_T_COST_PASSWORD
        || params.p_cost() < CURRENT_P_COST)
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
///
/// Side effects on successful verification:
/// - Updates `last_used_at` for the matching credential row.
/// - If the stored hash uses parameters weaker than the current Argon2 target
///   (see [`is_password_hash_weaker_than_current`]), re-hashes the password
///   with [`current_argon2_password`] and writes the new PHC string back to
///   `credentials.data`. Rehash failures are logged but never break the login.
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
            // Update last_used_at.
            let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
            conn.execute(
                "UPDATE credentials SET last_used_at = ?1 WHERE id = ?2",
                params![now, cred_id],
            )?;

            // Opportunistically upgrade the hash to current Argon2 params.
            // A failure here must never break login — log a warning and continue.
            match is_password_hash_weaker_than_current(&hash) {
                Ok(true) => match hash_password(password) {
                    Ok(new_hash) => {
                        if let Err(e) = conn.execute(
                            "UPDATE credentials SET data = ?1 WHERE id = ?2",
                            params![new_hash, cred_id],
                        ) {
                            tracing::warn!(
                                event = "argon2_rehash_failed",
                                user_id = %user_id,
                                error = %e,
                                "failed to write rehashed password",
                            );
                        } else {
                            tracing::info!(
                                event = "argon2_rehash",
                                user_id = %user_id,
                                "rehashed weak password hash",
                            );
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            event = "argon2_rehash_failed",
                            user_id = %user_id,
                            error = %e,
                            "failed to compute rehashed password",
                        );
                    }
                },
                Ok(false) => {}
                Err(e) => {
                    tracing::warn!(
                        event = "argon2_rehash_skipped",
                        user_id = %user_id,
                        error = %e,
                        "could not inspect stored hash params; skipping rehash",
                    );
                }
            }
            return Ok(true);
        }
    }
    Ok(false)
}

/// Replace a user's password. Deletes all existing password credentials and stores the new one.
pub fn change_password(
    conn: &rusqlite::Connection,
    user_id: &str,
    new_password: &str,
) -> Result<()> {
    conn.execute(
        "DELETE FROM credentials WHERE user_id = ?1 AND kind = 'password'",
        params![user_id],
    )?;
    store_password(conn, user_id, new_password)?;
    Ok(())
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
        // Recovery codes are high-value and single-use — hash with stronger params.
        let hash = hash_recovery_code(&code)?;
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

    // -- Argon2 params + rehash --------------------------------------------------

    /// Local shim: the inline tests share the public test helper so a single
    /// authoritative implementation defines what "legacy default params" means.
    fn legacy_default_hash(password: &str) -> Vec<u8> {
        legacy_argon2_default_hash_for_tests(password).expect("legacy hash helper")
    }

    #[test]
    fn current_argon2_password_uses_t1() {
        let a = current_argon2_password();
        let params = a.params();
        assert_eq!(params.m_cost(), CURRENT_M_COST);
        assert_eq!(params.t_cost(), CURRENT_T_COST_PASSWORD);
        assert_eq!(params.p_cost(), CURRENT_P_COST);
    }

    #[test]
    fn current_argon2_recovery_uses_stronger_t3() {
        let a = current_argon2_recovery();
        let params = a.params();
        assert_eq!(params.m_cost(), CURRENT_M_COST);
        assert_eq!(params.t_cost(), CURRENT_T_COST_RECOVERY);
        assert!(
            params.t_cost() >= 3,
            "recovery code params must have t_cost >= 3, got {}",
            params.t_cost()
        );
        assert_eq!(params.p_cost(), CURRENT_P_COST);
    }

    #[test]
    fn fresh_hash_password_embeds_current_params() {
        let hash = hash_password("hunter22hunter").unwrap();
        let hash_str = std::str::from_utf8(&hash).unwrap();
        let parsed = PasswordHash::new(hash_str).unwrap();
        let params = Params::try_from(&parsed).unwrap();
        assert_eq!(params.m_cost(), CURRENT_M_COST);
        assert_eq!(params.t_cost(), CURRENT_T_COST_PASSWORD);
        assert_eq!(params.p_cost(), CURRENT_P_COST);
    }

    #[test]
    fn argon2_default_hash_is_weaker_than_current() {
        let legacy = legacy_default_hash("hunter22hunter");
        assert!(is_password_hash_weaker_than_current(&legacy).unwrap());
    }

    #[test]
    fn current_hash_is_not_weaker() {
        let fresh = hash_password("hunter22hunter").unwrap();
        assert!(!is_password_hash_weaker_than_current(&fresh).unwrap());
    }

    #[test]
    fn verify_password_passes_old_params_hash() {
        // Sanity: verify_password reads stored params from the PHC string and
        // works regardless of which Argon2 instance hashed the input.
        let legacy = legacy_default_hash("hunter22hunter");
        assert!(verify_password("hunter22hunter", &legacy).unwrap());
        assert!(!verify_password("wrong-password", &legacy).unwrap());
    }

    #[test]
    fn verify_user_password_rehashes_weak_hash() {
        let (conn, user_id) = test_conn_with_user();
        let cred_id = Uuid::new_v4().to_string();
        let legacy = legacy_default_hash("hunter22hunter");
        conn.execute(
            "INSERT INTO credentials (id, user_id, kind, data, label) VALUES (?1, ?2, 'password', ?3, NULL)",
            params![cred_id, user_id, legacy.clone()],
        )
        .unwrap();

        // Verify succeeds and triggers rehash.
        assert!(verify_user_password(&conn, &user_id, "hunter22hunter").unwrap());

        // Row was rewritten with current params.
        let new_hash: Vec<u8> = conn
            .query_row(
                "SELECT data FROM credentials WHERE id = ?1",
                params![cred_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_ne!(legacy, new_hash, "credentials.data should have been rewritten");
        assert!(!is_password_hash_weaker_than_current(&new_hash).unwrap());

        // The new hash still verifies the same password.
        assert!(verify_password("hunter22hunter", &new_hash).unwrap());
    }

    #[test]
    fn verify_user_password_does_not_rewrite_current_param_hash() {
        let (conn, user_id) = test_conn_with_user();
        // store_password uses current params.
        let cred_id = store_password(&conn, &user_id, "hunter22hunter").unwrap();
        let before: Vec<u8> = conn
            .query_row(
                "SELECT data FROM credentials WHERE id = ?1",
                params![cred_id],
                |r| r.get(0),
            )
            .unwrap();

        assert!(verify_user_password(&conn, &user_id, "hunter22hunter").unwrap());

        let after: Vec<u8> = conn
            .query_row(
                "SELECT data FROM credentials WHERE id = ?1",
                params![cred_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(before, after, "current-param hash must not be rewritten");
    }

    #[test]
    fn recovery_code_hash_uses_stronger_params() {
        let (conn, user_id) = test_conn_with_user();
        let codes = generate_recovery_codes(&conn, &user_id, 2).unwrap();
        assert_eq!(codes.len(), 2);

        let hashes: Vec<Vec<u8>> = conn
            .prepare("SELECT code_hash FROM recovery_codes WHERE user_id = ?1")
            .unwrap()
            .query_map(params![user_id], |r| r.get::<_, Vec<u8>>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(!hashes.is_empty());
        for hash in &hashes {
            let hash_str = std::str::from_utf8(hash).unwrap();
            let parsed = PasswordHash::new(hash_str).unwrap();
            let params = Params::try_from(&parsed).unwrap();
            assert!(
                params.t_cost() >= CURRENT_T_COST_RECOVERY,
                "recovery code hash must have t_cost >= {} (got {})",
                CURRENT_T_COST_RECOVERY,
                params.t_cost()
            );
            assert!(
                params.m_cost() >= CURRENT_M_COST,
                "recovery code hash must have m_cost >= {} (got {})",
                CURRENT_M_COST,
                params.m_cost()
            );
        }
    }

    /// Latency check: keep the new params under the 500 ms budget on developer
    /// machines. Marked `#[ignore]` so CI can run it explicitly when calibrating;
    /// `cargo test -- --ignored` exercises it locally.
    #[test]
    #[ignore = "latency benchmark; run with --ignored"]
    fn hash_latency_under_500ms() {
        let start = std::time::Instant::now();
        let _ = hash_password("hunter22hunter").unwrap();
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_millis(500),
            "argon2 hash too slow: {elapsed:?}",
        );
    }
}
