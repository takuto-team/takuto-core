// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Envelope encryption helpers and master key bootstrap.
//!
//! Envelope scheme:
//!
//! ```text
//! plaintext --AEAD(DEK, nonce)--> ciphertext
//! DEK       --AEAD(MK, wnonce)--> wrapped_dek
//! ```
//!
//! - **AEAD**: `XChaCha20-Poly1305` — 256-bit key, 192-bit nonce, 128-bit tag.
//! - **DEK**: 32-byte random key per row, regenerated on every save. Never
//!   persisted in plaintext.
//! - **MK**: 32-byte master key. Sourced from `TAKUTO_SECRET_KEY` env var
//!   (64 hex chars) or `${data_dir}/secret.key` (raw 32 bytes, mode 0600).
//! - **Nonces**: 24 fresh random bytes per write (length-checked at deserialise
//!   time).
//!
//! This module ships seal/open and the key bootstrap; the per-user credential
//! CRUD layer consumes these primitives.

pub mod bundle;
pub mod error;
pub mod gh_client;
pub mod master_key;
pub mod pat_validation;
pub mod seal;

pub use error::AuthError;

pub use bundle::{
    SECRET_FILE_CLAUDE, SECRET_FILE_CODEX, SECRET_FILE_CURSOR, SECRET_FILE_GH,
    SECRET_FILE_OPENCODE, WORKER_SECRETS_MOUNTPOINT, WorkerSecretsBundle,
    build_for_endpoint as build_bundle_for_endpoint,
};
pub use gh_client::{GhClient, GhResponse, RealGhClient, SharedGhClient};
pub use master_key::{MasterKey, MasterKeySource, load_or_init_master_key};
pub use pat_validation::{PatValidationError, ValidatedPat, validate_pat};
pub use seal::{SealedBlob, open, seal};
