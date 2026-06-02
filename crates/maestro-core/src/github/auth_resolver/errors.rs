// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Error + secret-bearing value types for the `auth_resolver` module.
//!
//! Mechanical split out of the legacy single-file `auth_resolver.rs`. Holds:
//!
//! - [`GitAuthError`] — typed failure surface for the resolver, mapped to
//!   stable `credential_audit.error_code` strings via [`GitAuthError::code`].
//! - [`GitAuthResult`] — `Result<T, GitAuthError>` alias.
//! - [`SecretToken`] — small wrapper that redacts its bytes in `Debug` /
//!   `Display` printouts so logging a value never leaks a token.
//! - [`GitToken`] — what the resolver hands callers about to perform a git
//!   operation (bearer + author identity + source tag).
//! - [`auth_warning_payload`] — `(code, message)` builder used by the
//!   workflow engine to emit `WorkflowEvent::AuthWarning` rows.

use std::fmt;

use super::TokenSource;

// ---------------------------------------------------------------------------
// SecretToken: redacted Debug wrapper
// ---------------------------------------------------------------------------

/// Wraps a token string so the bytes never reach a `Debug` or `Display`
/// printout. Equivalent to `secrecy::SecretString` for the operations the
/// resolver needs — we roll our own to avoid adding a new dependency.
#[derive(Clone)]
pub struct SecretToken(String);

impl SecretToken {
    pub fn new(bytes: String) -> Self {
        Self(bytes)
    }
    /// Expose the token bytes. Callers should pass directly into a
    /// short-lived env var or command argument and never log the result.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretToken")
            .field("len", &self.0.len())
            .field("bytes", &"<redacted>")
            .finish()
    }
}

// ---------------------------------------------------------------------------
// GitToken: the resolver's return value
// ---------------------------------------------------------------------------

/// What the resolver hands back to a caller about to perform a git
/// operation. The bearer is wrapped so logging the struct doesn't leak the
/// token.
#[derive(Debug)]
pub struct GitToken {
    pub bearer: SecretToken,
    pub source: TokenSource,
    /// `Some(login)` when [`TokenSource::UserPat`]; `Some("maestro-bot[bot]")`
    /// when [`TokenSource::App`]. The driver uses this for git author env.
    pub author_name: Option<String>,
    /// `Some(<login>@users.noreply.github.com)` when [`TokenSource::UserPat`];
    /// `Some(<app_id>+maestro-bot[bot]@users.noreply.github.com)` when App.
    pub author_email: Option<String>,
    /// Pinned onto `PersistedWorkflowRecord.auth_pin` so a restored
    /// workflow still resolves the same credential row even after a
    /// deployment-wide provider switch invalidated newer rows.
    pub credential_row_id: Option<i64>,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Typed failures the resolver can surface. Each variant maps onto a stable
/// audit-log `error_code`.
#[derive(Debug, thiserror::Error)]
pub enum GitAuthError {
    #[error("UnauthenticatedGit: no GitHub auth source available for action {action} (user {user_id})")]
    UnauthenticatedGit {
        user_id: String,
        action: &'static str,
    },
    #[error("MasterKeyUnavailable: cannot unseal user PAT for user {user_id}")]
    MasterKeyUnavailable { user_id: String },
    #[error("sso_authorization_required: org={org} url={sso_url}")]
    SsoAuthorizationRequired { org: String, sso_url: String },
    /// Bubbled-up from the App token manager when the JWT / network path
    /// fails. The caller surfaces this as a step error; not retried here.
    #[error("GitHubAppTokenFetchFailed: {message}")]
    GitHubAppTokenFetchFailed { message: String },
    /// Bubbled up from the DB / decrypt layer. Always logged at the call
    /// site; bringing it through this enum keeps the typed error surface
    /// honest.
    #[error("ResolverInternal: {message}")]
    Internal { message: String },
}

impl GitAuthError {
    /// Stable code for `credential_audit.error_code` columns.
    pub fn code(&self) -> &'static str {
        match self {
            GitAuthError::UnauthenticatedGit { .. } => "unauthenticated_git",
            GitAuthError::MasterKeyUnavailable { .. } => "master_key_unavailable",
            GitAuthError::SsoAuthorizationRequired { .. } => "sso_authorization_required",
            GitAuthError::GitHubAppTokenFetchFailed { .. } => "github_app_token_fetch_failed",
            GitAuthError::Internal { .. } => "resolver_internal",
        }
    }
}

pub type GitAuthResult<T> = std::result::Result<T, GitAuthError>;

/// Build a `WorkflowEvent::AuthWarning`-shaped payload from a
/// [`GitAuthError`] for the dashboard to render. Pure helper so the engine
/// can call this from both the restore path and the resume path without
/// duplicating string-building logic.
pub fn auth_warning_payload(err: &GitAuthError) -> (&'static str, String) {
    match err {
        GitAuthError::SsoAuthorizationRequired { org, sso_url } => (
            "sso_authorization_required",
            format!("SSO authorization required for org {org}: authorize at {sso_url}"),
        ),
        GitAuthError::UnauthenticatedGit { .. } => (
            "unauthenticated_git",
            "GitHub authentication missing for this workflow's owner".to_string(),
        ),
        GitAuthError::MasterKeyUnavailable { .. } => (
            "master_key_unavailable",
            "Master key not loaded; per-user credentials cannot be unsealed".to_string(),
        ),
        GitAuthError::GitHubAppTokenFetchFailed { message } => (
            "github_app_token_fetch_failed",
            message.clone(),
        ),
        GitAuthError::Internal { message } => ("auth_warning", message.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_token_debug_does_not_leak_bytes() {
        let t = SecretToken::new("ghp_super_secret_token".into());
        let s = format!("{t:?}");
        assert!(!s.contains("ghp_super_secret_token"));
        assert!(s.contains("redacted"));
    }

    #[test]
    fn auth_warning_payload_maps_each_variant_to_stable_code() {
        let (c, _) = auth_warning_payload(&GitAuthError::SsoAuthorizationRequired {
            org: "acme".into(),
            sso_url: "u".into(),
        });
        assert_eq!(c, "sso_authorization_required");

        let (c, _) = auth_warning_payload(&GitAuthError::MasterKeyUnavailable {
            user_id: "u".into(),
        });
        assert_eq!(c, "master_key_unavailable");

        let (c, _) = auth_warning_payload(&GitAuthError::Internal {
            message: "boom".into(),
        });
        // Internal must not leak the message into the code field.
        assert!(!c.contains("boom"));
    }

    /// Lock-in test for `auth_warning_payload`'s `(category, message)` tuple
    /// across every `GitAuthError` variant. The dashboard renders these
    /// strings verbatim, so they form a wire contract that must not drift.
    ///
    /// One sample input per variant; payload-bearing variants
    /// (`SsoAuthorizationRequired`, `GitHubAppTokenFetchFailed`, `Internal`)
    /// exercise the interpolation/clone paths so the format strings are
    /// pinned, not just the category tags.
    #[test]
    fn lock_in_auth_warning_payload_shapes() {
        // Variant 1: SsoAuthorizationRequired — message interpolates org + sso_url.
        let err = GitAuthError::SsoAuthorizationRequired {
            org: "acme-corp".into(),
            sso_url: "https://github.com/orgs/acme-corp/sso?return_to=x".into(),
        };
        assert_eq!(
            auth_warning_payload(&err),
            (
                "sso_authorization_required",
                "SSO authorization required for org acme-corp: authorize at \
                 https://github.com/orgs/acme-corp/sso?return_to=x"
                    .to_string(),
            ),
        );

        // Variant 2: UnauthenticatedGit — static message; payload fields ignored.
        let err = GitAuthError::UnauthenticatedGit {
            user_id: "u-alice".into(),
            action: "push",
        };
        assert_eq!(
            auth_warning_payload(&err),
            (
                "unauthenticated_git",
                "GitHub authentication missing for this workflow's owner".to_string(),
            ),
        );

        // Variant 3: MasterKeyUnavailable — static message; user_id ignored.
        let err = GitAuthError::MasterKeyUnavailable {
            user_id: "u-bob".into(),
        };
        assert_eq!(
            auth_warning_payload(&err),
            (
                "master_key_unavailable",
                "Master key not loaded; per-user credentials cannot be unsealed".to_string(),
            ),
        );

        // Variant 4: GitHubAppTokenFetchFailed — message is the upstream string verbatim.
        let err = GitAuthError::GitHubAppTokenFetchFailed {
            message: "jwt: clock skew exceeds 60s".into(),
        };
        assert_eq!(
            auth_warning_payload(&err),
            (
                "github_app_token_fetch_failed",
                "jwt: clock skew exceeds 60s".to_string(),
            ),
        );

        // Variant 5: Internal — note this maps to the generic `auth_warning`
        // category (NOT `resolver_internal`, which is the `.code()` value).
        // The dashboard surfaces these as generic warnings so we don't leak
        // internal taxonomy to the UI.
        let err = GitAuthError::Internal {
            message: "sqlx: pool closed".into(),
        };
        assert_eq!(
            auth_warning_payload(&err),
            ("auth_warning", "sqlx: pool closed".to_string()),
        );
    }
}
