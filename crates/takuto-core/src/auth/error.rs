// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Typed errors for the auth subsystem: user CRUD, Argon2 password / recovery
//! code hashing + verification, session-cookie validation, and the registration
//! / login / change-password / recovery flows on the web side.
//!
//! Replaces the historical `TakutoError::Auth(String)` (the `*Str(String)`
//! deprecated shim was removed in the post-§8 #2 cleanup PR).
//! Each variant captures structured operation context —
//! usernames, user ids, the typed argon2 / serde / utf8 source error — instead
//! of `format!`-ed sentences. No site collapses to direct propagation here
//! (none of the original 33 sites wrapped an inner `TakutoError`; all are
//! domain-logic checks or foreign-error wraps).
//!
//! See `lore/audits/2026-05-21-clean-code.md` §8 #2 and
//! `lore/audits/2026-05-24-typed-errors-spec.md` for the architecture rules
//! this module follows.

use thiserror::Error;

/// Failures originating from user CRUD, Argon2 hashing/verification, and the
/// session / registration / recovery flows.
#[derive(Debug, Error)]
pub enum AuthError {
    // ── User CRUD (`db/users.rs`) ─────────────────────────────────────────
    /// Empty / whitespace-only username supplied to `create_user`.
    #[error("Username cannot be empty")]
    EmptyUsername,

    /// `INSERT INTO users` violated the UNIQUE(username) constraint.
    #[error("Username '{username}' already exists")]
    UsernameAlreadyExists { username: String },

    /// `SELECT … WHERE id = ?` returned no rows for the supplied id.
    #[error("User not found: {id}")]
    UserNotFound { id: String },

    /// Last-admin invariant guard: refused demote / suspend / delete because
    /// `op` would leave zero non-suspended admins in the table.
    #[error("Cannot {op}: this is the last non-suspended admin")]
    LastAdminLockout { op: &'static str },

    /// Race-condition guard: a user row was successfully `UPDATE`d but the
    /// subsequent `SELECT` returned no rows (concurrent delete).
    #[error("User disappeared after update")]
    UserDisappearedAfterUpdate,

    // ── Argon2 hashing (`db/credentials.rs`) ──────────────────────────────
    /// `getrandom::fill` failed while drawing CSPRNG bytes for an Argon2 salt.
    #[error("Failed to generate salt: {source}")]
    SaltGeneration {
        #[source]
        source: getrandom::Error,
    },

    /// `password_hash::SaltString::encode_b64` rejected the CSPRNG-derived
    /// bytes (effectively unreachable — included for completeness).
    #[error("Failed to encode salt: {source}")]
    SaltEncoding {
        #[source]
        source: argon2::password_hash::Error,
    },

    /// `argon2.hash_password(...)` failed for either a password or a recovery
    /// code. `kind` is a pinned `&'static str` ("password" or "recovery code").
    #[error("Failed to hash {kind}: {source}")]
    HashFailed {
        kind: &'static str,
        #[source]
        source: argon2::password_hash::Error,
    },

    /// The bytes stored in `credentials.data` (or `recovery_codes.code_hash`)
    /// are not valid UTF-8 — column corruption.
    #[error("Invalid stored hash encoding: {source}")]
    StoredHashEncoding {
        #[source]
        source: std::str::Utf8Error,
    },

    /// `PasswordHash::new(<str>)` rejected the stored PHC string format.
    #[error("Invalid password hash format: {source}")]
    PasswordHashFormat {
        #[source]
        source: argon2::password_hash::Error,
    },

    /// `Params::try_from(&parsed_hash)` could not extract typed Argon2
    /// parameters from the PHC string.
    #[error("Failed to read argon2 params: {source}")]
    ArgonParams {
        #[source]
        source: argon2::password_hash::Error,
    },

    // ── Web routes (`takuto-web/src/routes/{auth,admin}.rs`) ─────────────
    /// `verify_user_password` returned `false` during a password-change
    /// request.
    #[error("Current password is incorrect")]
    CurrentPasswordIncorrect,

    /// `db::sessions::get` returned `None` for a `db-`-prefixed session
    /// cookie (cookie present, no DB row).
    #[error("Invalid session")]
    InvalidSession,

    /// The recovery code submitted to `POST /api/auth/recover` did not match
    /// any unused `recovery_codes` row for the user.
    #[error("Invalid recovery code")]
    InvalidRecoveryCode,

    /// `POST /api/auth/register` rejected because `SELECT count(*) FROM users`
    /// returned non-zero (first-user setup is closed once a user exists).
    #[error("Registration is closed: users already exist. Use admin API to create new users.")]
    RegistrationClosed,

    /// Password length validation (`< 12 chars`) on register / change-password
    /// / admin set-password.
    #[error("Password must be at least 12 characters")]
    PasswordTooShort,

    // ── Web auth middleware (`takuto-web/src/auth.rs`) ───────────────────
    /// `serde_json::to_string(<session_data>)` failed while inserting a new
    /// DB session row.
    #[error("Failed to serialize session: {source}")]
    SessionSerialize {
        #[source]
        source: serde_json::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argon2_err() -> argon2::password_hash::Error {
        // Use a stable arm that does NOT carry a trailing operator detail
        // (Display = "invalid password format").
        argon2::password_hash::Error::Password
    }

    fn utf8_err() -> std::str::Utf8Error {
        // Use a heap-allocated Vec so clippy's `invalid_from_utf8` lint
        // can't const-eval the literal; the bytes are deliberately
        // invalid UTF-8 and we want the `Err` arm.
        let bytes: Vec<u8> = vec![0xff, 0xfe, 0xfd];
        std::str::from_utf8(&bytes).unwrap_err()
    }

    fn getrandom_err() -> getrandom::Error {
        getrandom::Error::UNSUPPORTED
    }

    fn serde_json_err() -> serde_json::Error {
        serde_json::from_str::<serde_json::Value>("{invalid").unwrap_err()
    }

    #[test]
    fn lock_in_auth_error_display() {
        let cases: Vec<(AuthError, String)> =
            vec![
            (AuthError::EmptyUsername, "Username cannot be empty".to_string()),
            (
                AuthError::UsernameAlreadyExists {
                    username: "alice".to_string(),
                },
                "Username 'alice' already exists".to_string(),
            ),
            (
                AuthError::UserNotFound {
                    id: "u-1".to_string(),
                },
                "User not found: u-1".to_string(),
            ),
            (
                AuthError::LastAdminLockout { op: "demote" },
                "Cannot demote: this is the last non-suspended admin".to_string(),
            ),
            (
                AuthError::UserDisappearedAfterUpdate,
                "User disappeared after update".to_string(),
            ),
            (
                AuthError::SaltGeneration {
                    source: getrandom_err(),
                },
                format!("Failed to generate salt: {}", getrandom_err()),
            ),
            (
                AuthError::SaltEncoding {
                    source: argon2_err(),
                },
                format!("Failed to encode salt: {}", argon2_err()),
            ),
            (
                AuthError::HashFailed {
                    kind: "password",
                    source: argon2_err(),
                },
                format!("Failed to hash password: {}", argon2_err()),
            ),
            (
                AuthError::StoredHashEncoding { source: utf8_err() },
                format!("Invalid stored hash encoding: {}", utf8_err()),
            ),
            (
                AuthError::PasswordHashFormat {
                    source: argon2_err(),
                },
                format!("Invalid password hash format: {}", argon2_err()),
            ),
            (
                AuthError::ArgonParams {
                    source: argon2_err(),
                },
                format!("Failed to read argon2 params: {}", argon2_err()),
            ),
            (
                AuthError::CurrentPasswordIncorrect,
                "Current password is incorrect".to_string(),
            ),
            (AuthError::InvalidSession, "Invalid session".to_string()),
            (
                AuthError::InvalidRecoveryCode,
                "Invalid recovery code".to_string(),
            ),
            (
                AuthError::RegistrationClosed,
                "Registration is closed: users already exist. Use admin API to create new users."
                    .to_string(),
            ),
            (
                AuthError::PasswordTooShort,
                "Password must be at least 12 characters".to_string(),
            ),
            (
                AuthError::SessionSerialize {
                    source: serde_json_err(),
                },
                format!("Failed to serialize session: {}", serde_json_err()),
            ),
        ];
        // Drift detection: bump cases.len() when a new variant lands.
        assert_eq!(cases.len(), 17);
        for (err, expected) in cases {
            assert_eq!(format!("{err}"), expected, "Display mismatch for {err:?}");
        }
    }

    #[test]
    fn lock_in_auth_error_into_takuto_error() {
        use crate::error::TakutoError;
        let cases: Vec<AuthError> = vec![
            AuthError::EmptyUsername,
            AuthError::UsernameAlreadyExists {
                username: "a".to_string(),
            },
            AuthError::UserNotFound {
                id: "u".to_string(),
            },
            AuthError::LastAdminLockout { op: "demote" },
            AuthError::UserDisappearedAfterUpdate,
            AuthError::SaltGeneration {
                source: getrandom_err(),
            },
            AuthError::SaltEncoding {
                source: argon2_err(),
            },
            AuthError::HashFailed {
                kind: "password",
                source: argon2_err(),
            },
            AuthError::StoredHashEncoding { source: utf8_err() },
            AuthError::PasswordHashFormat {
                source: argon2_err(),
            },
            AuthError::ArgonParams {
                source: argon2_err(),
            },
            AuthError::CurrentPasswordIncorrect,
            AuthError::InvalidSession,
            AuthError::InvalidRecoveryCode,
            AuthError::RegistrationClosed,
            AuthError::PasswordTooShort,
            AuthError::SessionSerialize {
                source: serde_json_err(),
            },
        ];
        assert_eq!(cases.len(), 17);
        for err in cases {
            let outer: TakutoError = err.into();
            assert!(
                matches!(outer, TakutoError::Auth(_)),
                "expected TakutoError::Auth, got {outer:?}"
            );
        }
    }
}
