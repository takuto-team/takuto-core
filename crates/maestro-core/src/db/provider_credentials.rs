// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! `user_provider_credentials` table — row shapes only.
//!
//! Phase 2a foundation: structs mirror the table for downstream callers to
//! reference. Insert / select / update / delete helpers land in Phase 2b
//! alongside the credential CRUD endpoints.

use serde::{Deserialize, Serialize};

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
