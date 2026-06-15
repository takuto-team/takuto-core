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
//!
//! ### Adapter contract
//!
//! Reads take `&DbAdapter`; writes take `&mut DbTransaction<'_>` so the
//! recover-flow (verify + consume + change_password + delete_user_sessions)
//! commits atomically. Pure-crypto helpers (`hash_password`,
//! `verify_password`, etc.) don't touch the DB.

use argon2::password_hash::SaltString;
use argon2::{Algorithm, Argon2, Params, PasswordHash, PasswordHasher, PasswordVerifier, Version};
use uuid::Uuid;

use crate::auth::AuthError;
use crate::db::{DbAdapter, DbTransaction, DbValue};
use crate::error::Result;

/// Current Argon2id `m_cost` (KiB) for both passwords and recovery codes.
const CURRENT_M_COST: u32 = 47104;
/// Current Argon2id `t_cost` (iterations) for passwords.
const CURRENT_T_COST_PASSWORD: u32 = 1;
/// Current Argon2id `t_cost` (iterations) for recovery codes — stronger because
/// recovery codes are single-use and high-value.
const CURRENT_T_COST_RECOVERY: u32 = 3;
/// Current Argon2id `p_cost` (parallelism) for both passwords and recovery codes.
const CURRENT_P_COST: u32 = 1;

/// Produce a password hash using `Argon2::default()` parameters. **Test
/// helper only** — public so sibling-crate tests can seed pre-upgrade
/// rows.
pub fn legacy_argon2_default_hash_for_tests(password: &str) -> Result<Vec<u8>> {
    let mut salt_bytes = [0u8; 16];
    getrandom::fill(&mut salt_bytes).map_err(|source| AuthError::SaltGeneration { source })?;
    let salt =
        SaltString::encode_b64(&salt_bytes).map_err(|source| AuthError::SaltEncoding { source })?;
    let argon2 = Argon2::default();
    let hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|source| AuthError::HashFailed {
            kind: "password",
            source,
        })?;
    Ok(hash.to_string().into_bytes())
}

fn current_argon2_password() -> Argon2<'static> {
    // SAFETY: `Params::new` rejects values outside Argon2's published
    // ranges; CURRENT_* constants are covered by `params_within_argon2_bounds`.
    let params = Params::new(
        CURRENT_M_COST,
        CURRENT_T_COST_PASSWORD,
        CURRENT_P_COST,
        None,
    )
    .expect("CURRENT_* constants are within Argon2id parameter bounds");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

fn current_argon2_recovery() -> Argon2<'static> {
    // SAFETY: same Argon2 bounds argument as `current_argon2_password`.
    let params = Params::new(
        CURRENT_M_COST,
        CURRENT_T_COST_RECOVERY,
        CURRENT_P_COST,
        None,
    )
    .expect("CURRENT_* constants are within Argon2id parameter bounds");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

/// Hash a password using Argon2id with the current parameters and a random salt.
pub fn hash_password(password: &str) -> Result<Vec<u8>> {
    let mut salt_bytes = [0u8; 16];
    getrandom::fill(&mut salt_bytes).map_err(|source| AuthError::SaltGeneration { source })?;
    let salt =
        SaltString::encode_b64(&salt_bytes).map_err(|source| AuthError::SaltEncoding { source })?;
    let argon2 = current_argon2_password();
    let hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|source| AuthError::HashFailed {
            kind: "password",
            source,
        })?;
    Ok(hash.to_string().into_bytes())
}

fn hash_recovery_code(code: &str) -> Result<Vec<u8>> {
    let mut salt_bytes = [0u8; 16];
    getrandom::fill(&mut salt_bytes).map_err(|source| AuthError::SaltGeneration { source })?;
    let salt =
        SaltString::encode_b64(&salt_bytes).map_err(|source| AuthError::SaltEncoding { source })?;
    let argon2 = current_argon2_recovery();
    let hash = argon2
        .hash_password(code.as_bytes(), &salt)
        .map_err(|source| AuthError::HashFailed {
            kind: "recovery code",
            source,
        })?;
    Ok(hash.to_string().into_bytes())
}

/// Verify a password against a stored Argon2 hash. The verifier reads
/// the PHC string's embedded parameters, so this works for any param
/// set that produced `stored_hash`.
pub fn verify_password(password: &str, stored_hash: &[u8]) -> Result<bool> {
    let hash_str = std::str::from_utf8(stored_hash)
        .map_err(|source| AuthError::StoredHashEncoding { source })?;
    let parsed_hash =
        PasswordHash::new(hash_str).map_err(|source| AuthError::PasswordHashFormat { source })?;
    let argon2 = current_argon2_password();
    Ok(argon2
        .verify_password(password.as_bytes(), &parsed_hash)
        .is_ok())
}

fn is_password_hash_weaker_than_current(stored_hash: &[u8]) -> Result<bool> {
    let hash_str = std::str::from_utf8(stored_hash)
        .map_err(|source| AuthError::StoredHashEncoding { source })?;
    let parsed =
        PasswordHash::new(hash_str).map_err(|source| AuthError::PasswordHashFormat { source })?;
    let params = Params::try_from(&parsed).map_err(|source| AuthError::ArgonParams { source })?;
    Ok(params.m_cost() < CURRENT_M_COST
        || params.t_cost() < CURRENT_T_COST_PASSWORD
        || params.p_cost() < CURRENT_P_COST)
}

/// Store a password credential for a user. Hashes the password before
/// storing. Inside a `DbTransaction` because `routes/auth/register.rs`
/// co-commits this with `users::create_user` (well, that's done by the
/// outer route's atomic flow).
pub async fn store_password(
    tx: &mut DbTransaction<'_>,
    user_id: &str,
    password: &str,
) -> Result<String> {
    let hash = hash_password(password)?;
    let cred_id = Uuid::new_v4().to_string();
    tx.execute(
        "INSERT INTO credentials (id, user_id, kind, data, label) VALUES (?, ?, 'password', ?, NULL)",
        vec![
            DbValue::Text(cred_id.clone()),
            DbValue::Text(user_id.to_string()),
            DbValue::Bytes(hash),
        ],
    )
    .await?;
    Ok(cred_id)
}

/// Verify a password for a user. Returns `true` if any stored password
/// credential matches. Side effects on success: bumps `last_used_at`,
/// opportunistically rehashes weak hashes.
///
/// Takes `&DbAdapter` — the side-effect writes are best-effort and
/// don't need to co-commit with anything; rehash failures are logged
/// but never break the login.
pub async fn verify_user_password(
    adapter: &DbAdapter,
    user_id: &str,
    password: &str,
) -> Result<bool> {
    let rows = adapter
        .query_all(
            "SELECT id, data FROM credentials WHERE user_id = ? AND kind = 'password'",
            vec![DbValue::Text(user_id.to_string())],
        )
        .await?;

    for r in &rows {
        let cred_id = r.get_text(0)?;
        let hash = r.get_bytes(1)?;
        if verify_password(password, &hash)? {
            // Update last_used_at — fire-and-forget but propagate errors.
            let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
            adapter
                .execute(
                    "UPDATE credentials SET last_used_at = ? WHERE id = ?",
                    vec![DbValue::Text(now), DbValue::Text(cred_id.clone())],
                )
                .await?;

            // Opportunistic rehash. Failures here MUST never break login.
            match is_password_hash_weaker_than_current(&hash) {
                Ok(true) => match hash_password(password) {
                    Ok(new_hash) => {
                        let cred_id_clone = cred_id.clone();
                        if let Err(e) = adapter
                            .execute(
                                "UPDATE credentials SET data = ? WHERE id = ?",
                                vec![DbValue::Bytes(new_hash), DbValue::Text(cred_id_clone)],
                            )
                            .await
                        {
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

/// Replace a user's password. Deletes all existing password credentials
/// and stores the new one. Inside a `DbTransaction` so the
/// recover-flow's verify+consume+change+delete sequence commits
/// atomically.
pub async fn change_password(
    tx: &mut DbTransaction<'_>,
    user_id: &str,
    new_password: &str,
) -> Result<()> {
    tx.execute(
        "DELETE FROM credentials WHERE user_id = ? AND kind = 'password'",
        vec![DbValue::Text(user_id.to_string())],
    )
    .await?;
    store_password(tx, user_id, new_password).await?;
    Ok(())
}

/// Generate a set of recovery codes and store their hashes. Returns the
/// plaintext codes (to be displayed to the user once). Inside a
/// `DbTransaction` so register/recover routes can co-commit with their
/// other writes.
pub async fn generate_recovery_codes(
    tx: &mut DbTransaction<'_>,
    user_id: &str,
    count: usize,
) -> Result<Vec<String>> {
    tx.execute(
        "DELETE FROM recovery_codes WHERE user_id = ?",
        vec![DbValue::Text(user_id.to_string())],
    )
    .await?;

    let mut codes = Vec::with_capacity(count);
    for _ in 0..count {
        let code = generate_code();
        let hash = hash_recovery_code(&code)?;
        let id = Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO recovery_codes (id, user_id, code_hash, used) VALUES (?, ?, ?, 0)",
            vec![
                DbValue::Text(id),
                DbValue::Text(user_id.to_string()),
                DbValue::Bytes(hash),
            ],
        )
        .await?;
        codes.push(code);
    }
    Ok(codes)
}

/// Verify a recovery code and mark it as used (single-use). Returns
/// `true` if valid and unused. Inside a `DbTransaction` so the
/// recover-flow can commit the consume + the follow-on password change
/// atomically.
pub async fn verify_and_consume_recovery_code(
    tx: &mut DbTransaction<'_>,
    user_id: &str,
    code: &str,
) -> Result<bool> {
    let rows = tx
        .query_all(
            "SELECT id, code_hash FROM recovery_codes WHERE user_id = ? AND used = 0",
            vec![DbValue::Text(user_id.to_string())],
        )
        .await?;
    for r in &rows {
        let code_id = r.get_text(0)?;
        let hash = r.get_bytes(1)?;
        if verify_password(code, &hash)? {
            tx.execute(
                "UPDATE recovery_codes SET used = 1 WHERE id = ?",
                vec![DbValue::Text(code_id)],
            )
            .await?;
            return Ok(true);
        }
    }
    Ok(false)
}

/// Delete all sessions for a user (for use on suspend/password change).
/// Inside a `DbTransaction` so the password-change flow commits the
/// session wipe atomically.
pub async fn delete_user_sessions(tx: &mut DbTransaction<'_>, user_id: &str) -> Result<()> {
    tx.execute(
        "DELETE FROM sessions WHERE user_id = ?",
        vec![DbValue::Text(user_id.to_string())],
    )
    .await?;
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
    use crate::db::adapter::DbAdapter;
    use crate::db::migrate::DialectAwareMigrationSource;
    use crate::db::pool::{DbBackend, DbPool};
    use sqlx::sqlite::SqlitePool;

    async fn fresh_adapter_with_user() -> (DbAdapter, String) {
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
        let user_id =
            crate::db::users::create_user(&adapter, "alice", crate::db::models::UserRole::User)
                .await
                .unwrap()
                .id;
        (adapter, user_id)
    }

    /// Helper: wrap a single write in a short transaction.
    async fn store_password_committed(
        adapter: &DbAdapter,
        user_id: &str,
        password: &str,
    ) -> String {
        let mut tx = adapter.begin().await.unwrap();
        let id = store_password(&mut tx, user_id, password).await.unwrap();
        tx.commit().await.unwrap();
        id
    }

    async fn generate_recovery_codes_committed(
        adapter: &DbAdapter,
        user_id: &str,
        count: usize,
    ) -> Vec<String> {
        let mut tx = adapter.begin().await.unwrap();
        let codes = generate_recovery_codes(&mut tx, user_id, count)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        codes
    }

    async fn verify_and_consume_recovery_code_committed(
        adapter: &DbAdapter,
        user_id: &str,
        code: &str,
    ) -> bool {
        let mut tx = adapter.begin().await.unwrap();
        let ok = verify_and_consume_recovery_code(&mut tx, user_id, code)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        ok
    }

    #[tokio::test]
    async fn password_hash_and_verify() {
        let hash = hash_password("secret123").unwrap();
        assert!(verify_password("secret123", &hash).unwrap());
        assert!(!verify_password("wrong", &hash).unwrap());
    }

    #[tokio::test]
    async fn store_and_verify_password() {
        let (a, user_id) = fresh_adapter_with_user().await;
        store_password_committed(&a, &user_id, "mypassword").await;
        assert!(
            verify_user_password(&a, &user_id, "mypassword")
                .await
                .unwrap()
        );
        assert!(!verify_user_password(&a, &user_id, "wrong").await.unwrap());
    }

    #[tokio::test]
    async fn recovery_codes_generated_and_verified() {
        let (a, user_id) = fresh_adapter_with_user().await;
        let codes = generate_recovery_codes_committed(&a, &user_id, 8).await;
        assert_eq!(codes.len(), 8);

        let first = &codes[0];
        assert!(verify_and_consume_recovery_code_committed(&a, &user_id, first).await);
        // Second use should fail (already consumed).
        assert!(!verify_and_consume_recovery_code_committed(&a, &user_id, first).await);
    }

    #[tokio::test]
    async fn recovery_code_wrong_code_rejected() {
        let (a, user_id) = fresh_adapter_with_user().await;
        let _ = generate_recovery_codes_committed(&a, &user_id, 4).await;
        assert!(!verify_and_consume_recovery_code_committed(&a, &user_id, "XXXX-YYYY").await);
    }

    #[tokio::test]
    async fn regenerate_recovery_codes_replaces_old() {
        let (a, user_id) = fresh_adapter_with_user().await;
        let codes1 = generate_recovery_codes_committed(&a, &user_id, 4).await;
        let codes2 = generate_recovery_codes_committed(&a, &user_id, 4).await;

        assert!(!verify_and_consume_recovery_code_committed(&a, &user_id, &codes1[0]).await);
        assert!(verify_and_consume_recovery_code_committed(&a, &user_id, &codes2[0]).await);
    }

    #[tokio::test]
    async fn delete_user_sessions_works() {
        let (a, user_id) = fresh_adapter_with_user().await;
        // Insert a fake session.
        a.execute(
            "INSERT INTO sessions (id, user_id, data, expires_at) VALUES ('s1', ?, X'00', '2099-01-01T00:00:00Z')",
            vec![DbValue::Text(user_id.clone())],
        )
        .await
        .unwrap();
        let row = a
            .query_one(
                "SELECT COUNT(*) FROM sessions WHERE user_id = ?",
                vec![DbValue::Text(user_id.clone())],
            )
            .await
            .unwrap();
        assert_eq!(row.get_i64(0).unwrap(), 1);

        let mut tx = a.begin().await.unwrap();
        delete_user_sessions(&mut tx, &user_id).await.unwrap();
        tx.commit().await.unwrap();

        let row = a
            .query_one(
                "SELECT COUNT(*) FROM sessions WHERE user_id = ?",
                vec![DbValue::Text(user_id.clone())],
            )
            .await
            .unwrap();
        assert_eq!(row.get_i64(0).unwrap(), 0);
    }

    #[tokio::test]
    async fn generated_code_format() {
        let code = generate_code();
        assert_eq!(code.len(), 9);
        assert_eq!(code.chars().nth(4), Some('-'));
    }

    #[tokio::test]
    async fn current_argon2_password_uses_t1() {
        let a = current_argon2_password();
        let params = a.params();
        assert_eq!(params.m_cost(), CURRENT_M_COST);
        assert_eq!(params.t_cost(), CURRENT_T_COST_PASSWORD);
        assert_eq!(params.p_cost(), CURRENT_P_COST);
    }

    #[tokio::test]
    async fn current_argon2_recovery_uses_stronger_t3() {
        let a = current_argon2_recovery();
        let params = a.params();
        assert_eq!(params.m_cost(), CURRENT_M_COST);
        assert_eq!(params.t_cost(), CURRENT_T_COST_RECOVERY);
        assert!(params.t_cost() >= 3);
        assert_eq!(params.p_cost(), CURRENT_P_COST);
    }

    #[tokio::test]
    async fn fresh_hash_password_embeds_current_params() {
        let hash = hash_password("hunter22hunter").unwrap();
        let hash_str = std::str::from_utf8(&hash).unwrap();
        let parsed = PasswordHash::new(hash_str).unwrap();
        let params = Params::try_from(&parsed).unwrap();
        assert_eq!(params.m_cost(), CURRENT_M_COST);
        assert_eq!(params.t_cost(), CURRENT_T_COST_PASSWORD);
        assert_eq!(params.p_cost(), CURRENT_P_COST);
    }

    fn legacy_default_hash(password: &str) -> Vec<u8> {
        legacy_argon2_default_hash_for_tests(password).expect("legacy hash helper")
    }

    #[tokio::test]
    async fn argon2_default_hash_is_weaker_than_current() {
        let legacy = legacy_default_hash("hunter22hunter");
        assert!(is_password_hash_weaker_than_current(&legacy).unwrap());
    }

    #[tokio::test]
    async fn current_hash_is_not_weaker() {
        let fresh = hash_password("hunter22hunter").unwrap();
        assert!(!is_password_hash_weaker_than_current(&fresh).unwrap());
    }

    #[tokio::test]
    async fn verify_password_passes_old_params_hash() {
        let legacy = legacy_default_hash("hunter22hunter");
        assert!(verify_password("hunter22hunter", &legacy).unwrap());
        assert!(!verify_password("wrong-password", &legacy).unwrap());
    }

    #[tokio::test]
    async fn verify_user_password_rehashes_weak_hash() {
        let (a, user_id) = fresh_adapter_with_user().await;
        let cred_id = Uuid::new_v4().to_string();
        let legacy = legacy_default_hash("hunter22hunter");
        a.execute(
            "INSERT INTO credentials (id, user_id, kind, data, label) VALUES (?, ?, 'password', ?, NULL)",
            vec![
                DbValue::Text(cred_id.clone()),
                DbValue::Text(user_id.clone()),
                DbValue::Bytes(legacy.clone()),
            ],
        )
        .await
        .unwrap();

        assert!(
            verify_user_password(&a, &user_id, "hunter22hunter")
                .await
                .unwrap()
        );

        let row = a
            .query_one(
                "SELECT data FROM credentials WHERE id = ?",
                vec![DbValue::Text(cred_id.clone())],
            )
            .await
            .unwrap();
        let new_hash = row.get_bytes(0).unwrap();
        assert_ne!(legacy, new_hash);
        assert!(!is_password_hash_weaker_than_current(&new_hash).unwrap());
        assert!(verify_password("hunter22hunter", &new_hash).unwrap());
    }

    #[tokio::test]
    async fn verify_user_password_does_not_rewrite_current_param_hash() {
        let (a, user_id) = fresh_adapter_with_user().await;
        let cred_id = store_password_committed(&a, &user_id, "hunter22hunter").await;
        let row = a
            .query_one(
                "SELECT data FROM credentials WHERE id = ?",
                vec![DbValue::Text(cred_id.clone())],
            )
            .await
            .unwrap();
        let before = row.get_bytes(0).unwrap();

        assert!(
            verify_user_password(&a, &user_id, "hunter22hunter")
                .await
                .unwrap()
        );

        let row = a
            .query_one(
                "SELECT data FROM credentials WHERE id = ?",
                vec![DbValue::Text(cred_id)],
            )
            .await
            .unwrap();
        let after = row.get_bytes(0).unwrap();
        assert_eq!(before, after);
    }

    #[tokio::test]
    async fn recovery_code_hash_uses_stronger_params() {
        let (a, user_id) = fresh_adapter_with_user().await;
        let codes = generate_recovery_codes_committed(&a, &user_id, 2).await;
        assert_eq!(codes.len(), 2);

        let rows = a
            .query_all(
                "SELECT code_hash FROM recovery_codes WHERE user_id = ?",
                vec![DbValue::Text(user_id)],
            )
            .await
            .unwrap();
        assert!(!rows.is_empty());
        for r in &rows {
            let hash = r.get_bytes(0).unwrap();
            let hash_str = std::str::from_utf8(&hash).unwrap();
            let parsed = PasswordHash::new(hash_str).unwrap();
            let params = Params::try_from(&parsed).unwrap();
            assert!(params.t_cost() >= CURRENT_T_COST_RECOVERY);
            assert!(params.m_cost() >= CURRENT_M_COST);
        }
    }

    /// Latency check. Marked `#[ignore]` so CI can run it explicitly.
    #[tokio::test]
    #[ignore = "latency benchmark; run with --ignored"]
    async fn hash_latency_under_500ms() {
        let start = std::time::Instant::now();
        let _ = hash_password("hunter22hunter").unwrap();
        let elapsed = start.elapsed();
        assert!(elapsed < std::time::Duration::from_millis(500));
    }
}
