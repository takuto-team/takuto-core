// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! `credential_audit` table — row shapes only.
//!
//! Phase 2a foundation: a separate audit trail for credential operations
//! (create / rotate / delete / validation_failed / invalidated_provider_switch).
//!
//! NOT to be confused with the general-purpose `audit_events` table reserved
//! for plan-03 (different team, different concern: this one is per-credential,
//! that one is per-user-action). Phase 2b adds the insert helper and routes
//! that emit rows here.

use serde::{Deserialize, Serialize};

/// Discriminator for the credential the row refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialAuditKind {
    AiProvider,
    GithubPat,
    /// Reserved — Phase 3+ may add per-session ttyd state under this kind.
    CursorSession,
}

impl CredentialAuditKind {
    pub fn as_str(self) -> &'static str {
        match self {
            CredentialAuditKind::AiProvider => "ai_provider",
            CredentialAuditKind::GithubPat => "github_pat",
            CredentialAuditKind::CursorSession => "cursor_session",
        }
    }
}

/// One row of credential audit history.
#[derive(Debug, Clone)]
pub struct CredentialAuditRow {
    pub id: i64,
    pub user_id: String,
    /// `None` for system actions (e.g. cascade invalidation on provider
    /// switch); `Some(admin_user_id)` for admin-impersonation paths.
    pub actor_user_id: Option<String>,
    pub kind: CredentialAuditKind,
    /// Provider name when `kind == AiProvider`; `None` for `GithubPat` /
    /// `CursorSession`.
    pub provider: Option<String>,
    /// `"created" | "rotated" | "deleted" | "validation_failed" | "invalidated_provider_switch"`.
    pub event: String,
    /// `"ok" | "error"`.
    pub outcome: String,
    /// Classified error code (never the raw upstream body) when
    /// `outcome == "error"`.
    pub error_code: Option<String>,
    /// ISO-8601 UTC timestamp.
    pub at: String,
}
