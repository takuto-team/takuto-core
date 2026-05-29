// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! PAT / SSO revalidation helpers for the `auth_resolver` module.
//!
//! Both entry points are free fns taking `&GitAuthResolver` so the resolver
//! itself stays orchestration-only. The corresponding `impl GitAuthResolver`
//! methods are thin one-line delegators (kept as methods to preserve every
//! existing `resolver.revalidate_*` call site — no caller-side edits).
//!
//! - [`revalidate_pat_for_workflow`] runs `gh api user` + per-org SSO probe
//!   and writes a `credential_audit` row on failure.
//! - [`revalidate_sso`] is the SSO-only check Phase 2b.3 fires at workflow
//!   start; other validation failures aren't fatal at this layer.

use crate::auth::{GhClient, PatValidationError};
use crate::db::credential_audit::{self, CredentialAuditKind};

use super::{GitAuthError, GitAuthResolver, GitAuthResult};

/// Phase 2b.3.x: re-validate a user's PAT against the live `gh` shim at
/// workflow restore / resume time. Returns `Ok(())` when:
/// - the user has no PAT row (App-only / Missing modes — the App side
///   handles its own token rotation via the background writer), OR
/// - the PAT still validates against `gh api user` AND every org in
///   `orgs` accepts it (no `X-GitHub-SSO` block).
///
/// On failure, writes a `credential_audit` row with
/// `event = "validation_failed", outcome = "error", error_code = <code>`
/// before returning the typed error so the workflow driver can surface
/// a `WorkflowEvent::AuthWarning`.
pub async fn revalidate_pat_for_workflow(
    resolver: &GitAuthResolver,
    user_id: &str,
    gh: &dyn GhClient,
    orgs: &[String],
) -> GitAuthResult<()> {
    // Skip silently for App-only / Missing modes.
    if !resolver.user_has_pat(user_id).await? {
        return Ok(());
    }
    let pat = resolver.unseal_user_pat(user_id).await?;
    let result = crate::auth::validate_pat(gh, pat.expose(), orgs).await;
    if let Err(ref e) = result {
        // Audit the failure.
        let code = match e {
            PatValidationError::InvalidPat => "invalid_pat",
            PatValidationError::InsufficientScopes { .. } => "insufficient_scopes",
            PatValidationError::SsoAuthorizationRequired { .. } => "sso_authorization_required",
            PatValidationError::Transport(_) => "gh_transport_error",
        };
        // Plan-11 step 3 cluster B: credential_audit on the adapter.
        let _ = credential_audit::log(
            resolver.db().adapter(),
            user_id,
            None,
            CredentialAuditKind::GithubPat,
            None,
            "validation_failed",
            "error",
            Some(code),
        )
        .await;
    }
    match result {
        Ok(_) => Ok(()),
        Err(PatValidationError::SsoAuthorizationRequired { org, sso_url }) => {
            Err(GitAuthError::SsoAuthorizationRequired { org, sso_url })
        }
        Err(PatValidationError::InvalidPat) => Err(GitAuthError::Internal {
            message: "PAT no longer valid (revoked or expired)".into(),
        }),
        Err(PatValidationError::InsufficientScopes { missing }) => Err(GitAuthError::Internal {
            message: format!("PAT lost required scopes; missing: {}", missing.join(", ")),
        }),
        Err(PatValidationError::Transport(m)) => Err(GitAuthError::Internal {
            message: format!("PAT revalidation transport error: {m}"),
        }),
    }
}

/// Phase 2b.3 calls this at workflow start to re-check SSO authorisation
/// for every org the workflow will touch. Phase 2b.2 only exposes it;
/// the driver invocation lands later.
pub async fn revalidate_sso(
    resolver: &GitAuthResolver,
    user_id: &str,
    gh: &dyn GhClient,
    orgs: &[String],
) -> GitAuthResult<()> {
    // No PAT → nothing to revalidate. Return Ok so callers don't have to
    // branch on mode; SSO only matters for PAT-bearing flows.
    if !resolver.user_has_pat(user_id).await? {
        return Ok(());
    }
    let pat = resolver.unseal_user_pat(user_id).await?;
    match crate::auth::validate_pat(gh, pat.expose(), orgs).await {
        Ok(_) => Ok(()),
        Err(PatValidationError::SsoAuthorizationRequired { org, sso_url }) => {
            Err(GitAuthError::SsoAuthorizationRequired { org, sso_url })
        }
        // Other validation failures aren't fatal at this layer — they'll
        // surface again when a subsequent action tries to use the PAT.
        // The SSO check is specifically about org access loss.
        Err(other) => Err(GitAuthError::Internal {
            message: format!("PAT revalidation failed: {other:?}"),
        }),
    }
}
