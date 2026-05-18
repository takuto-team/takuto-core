// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! GitHub PAT validation — translates raw `GhClient` responses into the
//! typed outcomes the credential-save handler relies on.
//!
//! Source of truth: tmp/multi-agents/04_architecture.md §4.3 (PAT validation
//! at save AND at workflow start — Phase 2b.1 ships the save half; A4).
//!
//! Three steps:
//!
//! - `gh api -i user` with `Authorization: token <pat>` — captures `login`
//!   plus scopes from the `X-OAuth-Scopes` response header. 401/403 yields
//!   the typed `InvalidPat` error.
//! - Scope check: classic `repo` OR fine-grained
//!   `contents:write + pull_requests:write + issues:read`.
//! - SSO check: for each org provided by the caller, `gh api orgs/<org>` —
//!   a 403 with an `X-GitHub-SSO: required; url=…` header is the documented
//!   SSO-block signature.

use serde_json::Value;

use crate::auth::gh_client::{GhClient, GhResponse};

/// Successful validation outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedPat {
    pub login: String,
    pub scopes: Vec<String>,
}

/// Stable error codes mirrored on the wire as `{ "error": "<code>" }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatValidationError {
    /// `gh api user` returned 401/403 or empty login.
    InvalidPat,
    /// PAT is alive but missing the required scope set.
    InsufficientScopes { missing: Vec<String> },
    /// Org is SSO-protected and the PAT has not been authorised for it.
    SsoAuthorizationRequired { org: String, sso_url: String },
    /// Network / spawn / parse failure — distinct from "bad PAT".
    Transport(String),
}

impl PatValidationError {
    pub fn code(&self) -> &'static str {
        match self {
            PatValidationError::InvalidPat => "invalid_pat",
            PatValidationError::InsufficientScopes { .. } => "insufficient_scopes",
            PatValidationError::SsoAuthorizationRequired { .. } => "sso_authorization_required",
            PatValidationError::Transport(_) => "gh_transport_error",
        }
    }
}

/// Classic-PAT required scope; sufficient on its own.
const CLASSIC_REQUIRED: &str = "repo";
/// Fine-grained PAT required scope set; all three must be present.
const FINE_GRAINED_REQUIRED: &[&str] = &[
    "contents:write",
    "pull_requests:write",
    "issues:read",
];

/// Run the full PAT validation flow against the supplied `GhClient`.
///
/// `orgs` is the set of orgs to SSO-check; pass `&[]` when the deployment
/// has no org workflows yet (single-user installs). The PAT bytes are never
/// stored — they live as a borrowed slice for the duration of this call.
pub async fn validate_pat(
    gh: &dyn GhClient,
    pat: &str,
    orgs: &[String],
) -> Result<ValidatedPat, PatValidationError> {
    if pat.trim().is_empty() {
        return Err(PatValidationError::InvalidPat);
    }

    let resp = gh
        .api_user(pat)
        .await
        .map_err(PatValidationError::Transport)?;
    if resp.status == 401 || resp.status == 403 || resp.status == 404 {
        return Err(PatValidationError::InvalidPat);
    }
    if resp.status >= 400 {
        return Err(PatValidationError::Transport(format!(
            "gh api user returned {}: {}",
            resp.status,
            truncate_for_log(&resp.body)
        )));
    }

    let parsed: Value = serde_json::from_str(&resp.body)
        .map_err(|e| PatValidationError::Transport(format!("user JSON parse: {e}")))?;
    let login = parsed
        .get("login")
        .and_then(Value::as_str)
        .map(|s| s.to_string())
        .ok_or(PatValidationError::InvalidPat)?;
    if login.is_empty() {
        return Err(PatValidationError::InvalidPat);
    }

    // Scopes. Classic PATs use `X-OAuth-Scopes`; fine-grained PATs use
    // `X-Accepted-OAuth-Scopes` to advertise *available* scopes, but the
    // header set that proves possession of write access is the actual
    // `X-OAuth-Scopes` so we read that. Empty header → no scopes asserted.
    let scopes_hdr = resp.header("X-OAuth-Scopes").unwrap_or("").to_string();
    let scopes = parse_scopes_header(&scopes_hdr);
    check_scopes(&scopes)?;

    // SSO check per org. Stop at the first sso_required hit — the UI shows
    // one authorise button at a time anyway.
    for org in orgs {
        if org.trim().is_empty() {
            continue;
        }
        let org_resp = gh
            .api_org(pat, org)
            .await
            .map_err(PatValidationError::Transport)?;
        check_org_sso(&org_resp, org)?;
    }

    Ok(ValidatedPat { login, scopes })
}

/// Parse `X-OAuth-Scopes: repo, read:org` style header into a sorted token
/// list. Whitespace is trimmed; empty tokens are skipped.
fn parse_scopes_header(header: &str) -> Vec<String> {
    header
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn check_scopes(scopes: &[String]) -> Result<(), PatValidationError> {
    let has_classic = scopes.iter().any(|s| s == CLASSIC_REQUIRED);
    if has_classic {
        return Ok(());
    }
    let missing: Vec<String> = FINE_GRAINED_REQUIRED
        .iter()
        .filter(|req| !scopes.iter().any(|s| s == **req))
        .map(|s| s.to_string())
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(PatValidationError::InsufficientScopes { missing })
    }
}

fn check_org_sso(resp: &GhResponse, org: &str) -> Result<(), PatValidationError> {
    if resp.status == 200 {
        return Ok(());
    }
    if resp.status == 403
        && let Some(sso_hdr) = resp.header("X-GitHub-SSO")
    {
        // Header format: `required; url=https://github.com/orgs/<org>/sso?...`
        let url = sso_hdr
            .split(';')
            .find_map(|part| {
                let trimmed = part.trim();
                trimmed.strip_prefix("url=").map(|u| u.trim().to_string())
            })
            .unwrap_or_else(|| format!("https://github.com/orgs/{org}/sso"));
        return Err(PatValidationError::SsoAuthorizationRequired {
            org: org.to_string(),
            sso_url: url,
        });
    }
    if resp.status == 404 {
        // Org doesn't exist (or PAT can't see it). Treat as "no SSO block,
        // but no access either" — the dashboard will surface this as a
        // permissions issue later. For now, accept the validation so users
        // working in personal repos aren't gated.
        return Ok(());
    }
    if resp.status == 401 {
        return Err(PatValidationError::InvalidPat);
    }
    // Anything else: pass — we don't want to block PAT save on a transient
    // upstream hiccup. Workflow start re-validates per §4.3 call-site 2.
    Ok(())
}

fn truncate_for_log(s: &str) -> String {
    if s.len() <= 160 {
        s.to_string()
    } else {
        let mut t = s[..160].to_string();
        t.push('…');
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::gh_client::GhResponse;
    use std::sync::Mutex;

    /// Mock GhClient that returns pre-canned responses.
    struct MockGh {
        user: Mutex<Option<Result<GhResponse, String>>>,
        org: Mutex<Option<Result<GhResponse, String>>>,
    }

    impl MockGh {
        fn user_ok(login: &str, scopes: &str) -> Self {
            let body = format!("{{\"login\":\"{login}\"}}");
            Self {
                user: Mutex::new(Some(Ok(GhResponse {
                    status: 200,
                    headers: vec![("X-OAuth-Scopes".into(), scopes.into())],
                    body,
                }))),
                org: Mutex::new(Some(Ok(GhResponse {
                    status: 200,
                    headers: vec![],
                    body: "{}".into(),
                }))),
            }
        }

        fn user(resp: GhResponse) -> Self {
            Self {
                user: Mutex::new(Some(Ok(resp))),
                org: Mutex::new(Some(Ok(GhResponse {
                    status: 200,
                    headers: vec![],
                    body: "{}".into(),
                }))),
            }
        }

        fn with_org(mut self, resp: GhResponse) -> Self {
            self.org = Mutex::new(Some(Ok(resp)));
            self
        }
    }

    #[async_trait::async_trait]
    impl GhClient for MockGh {
        async fn api_user(&self, _pat: &str) -> Result<GhResponse, String> {
            self.user.lock().unwrap().clone().expect("no canned user")
        }
        async fn api_org(&self, _pat: &str, _org: &str) -> Result<GhResponse, String> {
            self.org.lock().unwrap().clone().expect("no canned org")
        }
    }

    #[tokio::test]
    async fn validate_classic_pat_with_repo_scope_succeeds() {
        let gh = MockGh::user_ok("alice", "repo, read:org");
        let v = validate_pat(&gh, "ghp_test", &[]).await.unwrap();
        assert_eq!(v.login, "alice");
        assert!(v.scopes.contains(&"repo".to_string()));
    }

    #[tokio::test]
    async fn validate_fine_grained_with_full_scope_set_succeeds() {
        let gh = MockGh::user_ok(
            "alice",
            "contents:write, pull_requests:write, issues:read",
        );
        let v = validate_pat(&gh, "github_pat_test", &[]).await.unwrap();
        assert_eq!(v.login, "alice");
    }

    #[tokio::test]
    async fn validate_rejects_401_as_invalid_pat() {
        let gh = MockGh::user(GhResponse {
            status: 401,
            headers: vec![],
            body: "{}".into(),
        });
        let err = validate_pat(&gh, "bogus", &[]).await.unwrap_err();
        assert!(matches!(err, PatValidationError::InvalidPat));
        assert_eq!(err.code(), "invalid_pat");
    }

    #[tokio::test]
    async fn validate_rejects_403_as_invalid_pat() {
        let gh = MockGh::user(GhResponse {
            status: 403,
            headers: vec![],
            body: "{}".into(),
        });
        let err = validate_pat(&gh, "bogus", &[]).await.unwrap_err();
        assert!(matches!(err, PatValidationError::InvalidPat));
    }

    #[tokio::test]
    async fn validate_rejects_empty_pat_without_calling_gh() {
        let gh = MockGh::user_ok("alice", "repo");
        let err = validate_pat(&gh, "", &[]).await.unwrap_err();
        assert!(matches!(err, PatValidationError::InvalidPat));
    }

    #[tokio::test]
    async fn validate_rejects_missing_repo_scope() {
        let gh = MockGh::user_ok("alice", "read:org");
        let err = validate_pat(&gh, "ghp", &[]).await.unwrap_err();
        match err {
            PatValidationError::InsufficientScopes { missing } => {
                assert!(missing.contains(&"contents:write".to_string()));
            }
            other => panic!("expected InsufficientScopes; got {other:?}"),
        }
    }

    #[tokio::test]
    async fn validate_rejects_partial_fine_grained_scopes() {
        let gh = MockGh::user_ok("alice", "contents:write");
        let err = validate_pat(&gh, "github_pat", &[]).await.unwrap_err();
        match err {
            PatValidationError::InsufficientScopes { missing } => {
                assert!(missing.contains(&"pull_requests:write".to_string()));
                assert!(missing.contains(&"issues:read".to_string()));
            }
            other => panic!("expected InsufficientScopes; got {other:?}"),
        }
    }

    #[tokio::test]
    async fn validate_sso_required_when_org_returns_403_with_header() {
        let gh = MockGh::user_ok("alice", "repo").with_org(GhResponse {
            status: 403,
            headers: vec![(
                "X-GitHub-SSO".into(),
                "required; url=https://github.com/orgs/foo/sso?return_to=x".into(),
            )],
            body: "{}".into(),
        });
        let err = validate_pat(&gh, "ghp", &["foo".to_string()])
            .await
            .unwrap_err();
        match err {
            PatValidationError::SsoAuthorizationRequired { org, sso_url } => {
                assert_eq!(org, "foo");
                assert!(sso_url.starts_with("https://github.com/orgs/foo/sso"));
            }
            other => panic!("expected SsoAuthorizationRequired; got {other:?}"),
        }
    }

    #[tokio::test]
    async fn validate_passes_when_org_returns_404() {
        // Org not visible to the PAT — not blocked at this layer; later
        // workflow-start re-validates.
        let gh = MockGh::user_ok("alice", "repo").with_org(GhResponse {
            status: 404,
            headers: vec![],
            body: "{}".into(),
        });
        let v = validate_pat(&gh, "ghp", &["unknown-org".to_string()])
            .await
            .unwrap();
        assert_eq!(v.login, "alice");
    }

    #[tokio::test]
    async fn validate_skips_empty_orgs_in_list() {
        let gh = MockGh::user_ok("alice", "repo");
        let v = validate_pat(&gh, "ghp", &["".to_string()]).await.unwrap();
        assert_eq!(v.login, "alice");
    }

    #[tokio::test]
    async fn validate_user_endpoint_transport_error_is_distinct_from_invalid() {
        struct Broken;
        #[async_trait::async_trait]
        impl GhClient for Broken {
            async fn api_user(&self, _pat: &str) -> Result<GhResponse, String> {
                Err("spawn gh failed: not found".into())
            }
            async fn api_org(&self, _pat: &str, _org: &str) -> Result<GhResponse, String> {
                unreachable!()
            }
        }
        let err = validate_pat(&Broken, "ghp", &[]).await.unwrap_err();
        match err {
            PatValidationError::Transport(_) => {}
            other => panic!("expected Transport; got {other:?}"),
        }
    }
}
