// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! `user_github_credentials` table — row shapes only.
//!
//! Phase 2a foundation: structs mirror the table for downstream callers to
//! reference. CRUD helpers land in Phase 2b alongside the
//! `POST /api/users/me/github-pat` endpoint.

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
