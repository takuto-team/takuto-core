// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Phase 2b.2 — `GitAuthResolver`. Picks the right token (GitHub App
//! installation token vs. per-user PAT) for a given [`GitAction`], based on
//! the per-user mode matrix in `tmp/multi-agents/04_architecture.md §4.2`.
//!
//! Scope: resolver + git-author plumbing. Worker container injection and
//! the per-workflow `auth_pin` ship in Phase 2b.3.
//!
//! Decision matrix (verbatim from arch doc §4.2):
//!
//! | `GitAction`                      | Mode B (App + PAT)            | Mode A (App only) | Mode C (PAT only) | Missing |
//! |----------------------------------|-------------------------------|-------------------|-------------------|---------|
//! | `Clone` / `Fetch`                | App                           | App               | UserPat           | Err     |
//! | `Push` (`attribute_commits=true`)| UserPat                       | App               | UserPat           | Err     |
//! | `Push` (`attribute_commits=false`)| App                          | App               | UserPat           | Err     |
//! | `PullRequestCreate`              | UserPat                       | App               | UserPat           | Err     |
//! | `PullRequestComment` / `Review`  | UserPat                       | App               | UserPat           | Err     |
//! | `IssueComment`                   | UserPat                       | App               | UserPat           | Err     |
//! | `WebhookEventIngest`             | App                           | App               | UserPat           | Err     |

use std::path::PathBuf;
use std::sync::Arc;

use crate::auth::{open, GhClient, SealedBlob};
use crate::db::github_credentials;
use crate::db::Database;
use crate::github_app::GitHubAppTokenManager;

pub mod audit;
pub mod decision;
pub mod errors;
pub mod validator;

pub use decision::{decide_token_source, GitAction, GithubAuthMode, TokenSource};
pub use errors::{auth_warning_payload, GitAuthError, GitAuthResult, GitToken, SecretToken};

// ---------------------------------------------------------------------------
// Resolver
// ---------------------------------------------------------------------------

/// Picks the right token per-action per-user per the matrix at the top of
/// the file. Holds a [`Database`] handle (for the user PAT row) and an
/// optional [`GitHubAppTokenManager`] (for installation tokens). The
/// resolver is `Clone`-cheap because both fields are `Arc`-shared.
#[derive(Clone)]
pub struct GitAuthResolver {
    db: Database,
    app: Option<Arc<GitHubAppTokenManager>>,
    /// `cwd` passed to `GitHubAppTokenManager::get_installation_token`. Set
    /// once at construction time; defaults to `/workspaces` (matches the
    /// existing `do_clone` call site). The path only matters because the
    /// underlying `gh` invocation needs a cwd.
    app_token_cwd: PathBuf,
}

impl GitAuthResolver {
    /// Build a resolver. `app` is `None` when no `[github]` App is configured.
    pub fn new(db: Database, app: Option<Arc<GitHubAppTokenManager>>) -> Self {
        Self {
            db,
            app,
            app_token_cwd: PathBuf::from("/workspaces"),
        }
    }

    /// Override the cwd passed to `gh` for App-token fetches. Tests use
    /// this to avoid hitting `/workspaces`.
    pub fn with_app_token_cwd(mut self, cwd: PathBuf) -> Self {
        self.app_token_cwd = cwd;
        self
    }

    /// Return the user's effective GitHub auth mode.
    ///
    /// Pure with respect to the network: only reads the local DB. Async
    /// because `tokio::sync::Mutex` requires it — there's no DB I/O beyond
    /// the rusqlite query under the lock.
    pub async fn mode_for_user(&self, user_id: &str) -> GitAuthResult<GithubAuthMode> {
        let app_configured = self.app.is_some();
        let pat_present = self.user_has_pat(user_id).await?;
        let mode = match (app_configured, pat_present) {
            (true, true) => GithubAuthMode::AppPlusPat,
            (true, false) => GithubAuthMode::AppOnly,
            (false, true) => GithubAuthMode::PatOnly,
            (false, false) => GithubAuthMode::Missing,
        };
        Ok(mode)
    }

    /// Pick + materialise a token for `(action, user_id)`.
    pub async fn token_for(
        &self,
        action: GitAction,
        user_id: &str,
    ) -> GitAuthResult<GitToken> {
        let mode = self.mode_for_user(user_id).await?;
        let attribute_commits = self.attribute_commits(user_id).await?;
        let source = decide_token_source(action, mode, attribute_commits)?
            // None means "neither App nor PAT for this user"
            .ok_or_else(|| GitAuthError::UnauthenticatedGit {
                user_id: user_id.to_string(),
                action: action.as_str(),
            })?;

        match source {
            TokenSource::App => self.materialise_app_token().await,
            TokenSource::UserPat => self.materialise_user_pat(user_id).await,
        }
    }

    /// Phase 2b.3.x: re-validate a user's PAT against the live `gh` shim at
    /// workflow restore / resume time. Thin delegator to
    /// [`validator::revalidate_pat_for_workflow`] — see that fn for
    /// behaviour.
    pub async fn revalidate_pat_for_workflow(
        &self,
        user_id: &str,
        gh: &dyn GhClient,
        orgs: &[String],
    ) -> GitAuthResult<()> {
        validator::revalidate_pat_for_workflow(self, user_id, gh, orgs).await
    }

    /// Phase 2b.3: SSO-only revalidation. Thin delegator to
    /// [`validator::revalidate_sso`] — see that fn for behaviour.
    pub async fn revalidate_sso(
        &self,
        user_id: &str,
        gh: &dyn GhClient,
        orgs: &[String],
    ) -> GitAuthResult<()> {
        validator::revalidate_sso(self, user_id, gh, orgs).await
    }

    // ── Internals ─────────────────────────────────────────────────────────

    /// Sibling-module access to the `Database` handle. Used by
    /// [`validator`] for the `credential_audit::log` write on validation
    /// failure. Crate-external callers must not depend on this — the
    /// resolver's public surface is `mode_for_user` / `token_for` /
    /// `revalidate_*`.
    pub(super) fn db(&self) -> &Database {
        &self.db
    }

    pub(super) async fn user_has_pat(&self, user_id: &str) -> GitAuthResult<bool> {
        let conn = self.db.conn().lock().await;
        let row = github_credentials::find(&conn, user_id).map_err(|e| GitAuthError::Internal {
            message: format!("github_credentials::find failed: {e}"),
        })?;
        Ok(row.is_some())
    }

    async fn attribute_commits(&self, user_id: &str) -> GitAuthResult<bool> {
        let conn = self.db.conn().lock().await;
        let row = github_credentials::find(&conn, user_id).map_err(|e| GitAuthError::Internal {
            message: format!("github_credentials::find failed: {e}"),
        })?;
        // Default true when no row — the matrix treats missing-PAT users as
        // "we'd attribute commits if they had one", which folds cleanly when
        // the chooser falls back to App regardless.
        Ok(row.map(|r| r.sign_commits).unwrap_or(true))
    }

    pub(super) async fn unseal_user_pat(&self, user_id: &str) -> GitAuthResult<SecretToken> {
        let mk = self.db.master_key().ok_or_else(|| {
            GitAuthError::MasterKeyUnavailable {
                user_id: user_id.to_string(),
            }
        })?.key.clone();
        let conn = self.db.conn().lock().await;
        let row = github_credentials::find(&conn, user_id)
            .map_err(|e| GitAuthError::Internal {
                message: format!("github_credentials::find failed: {e}"),
            })?
            .ok_or_else(|| GitAuthError::UnauthenticatedGit {
                user_id: user_id.to_string(),
                action: "unseal_user_pat",
            })?;
        // Drop the lock before doing CPU-bound AEAD work.
        drop(conn);
        let sealed = SealedBlob {
            ciphertext: row.ciphertext,
            nonce: row.nonce,
            wrapped_dek: row.wrapped_dek,
            wnonce: row.wnonce,
        };
        let plaintext = open(&mk, &sealed).map_err(|e| GitAuthError::Internal {
            message: format!("open(user_pat) failed: {e}"),
        })?;
        let s = String::from_utf8(plaintext).map_err(|_| GitAuthError::Internal {
            message: "user PAT plaintext is not UTF-8".into(),
        })?;
        Ok(SecretToken::new(s))
    }

    async fn materialise_app_token(&self) -> GitAuthResult<GitToken> {
        let app = self
            .app
            .as_ref()
            .ok_or_else(|| GitAuthError::Internal {
                message: "App-path selected but no GitHubAppTokenManager configured".into(),
            })?;
        let token = app
            .get_installation_token(&self.app_token_cwd)
            .await
            .map_err(|e| GitAuthError::GitHubAppTokenFetchFailed {
                message: e.to_string(),
            })?;
        Ok(GitToken {
            bearer: SecretToken::new(token),
            source: TokenSource::App,
            author_name: Some(app.bot_name().to_string()),
            author_email: Some(app.bot_email()),
            credential_row_id: None,
        })
    }

    async fn materialise_user_pat(&self, user_id: &str) -> GitAuthResult<GitToken> {
        let pat = self.unseal_user_pat(user_id).await?;
        // Read the login + row id under the same lock; bump last_used_at for
        // the audit debounce.
        let (login, row_id, last_used) = {
            let conn = self.db.conn().lock().await;
            let row = github_credentials::find(&conn, user_id)
                .map_err(|e| GitAuthError::Internal {
                    message: format!("github_credentials::find failed: {e}"),
                })?
                .ok_or_else(|| GitAuthError::UnauthenticatedGit {
                    user_id: user_id.to_string(),
                    action: "materialise_user_pat",
                })?;
            // user_github_credentials uses user_id as the PK; the
            // `credential_row_id` we expose for Phase 2b.3's auth_pin is a
            // stable derivation. None for now — there's no integer id col
            // (the table is keyed by user_id). Phase 2b.3 redefines this
            // field; for now we leave it None.
            (row.github_login, None::<i64>, row.last_validated_at)
        };

        // Audit "used" if this is the first use within ~60s. We co-opt
        // `last_validated_at` as a debounce signal (Phase 2b.3 may switch
        // this to a dedicated last_used_at column on user_github_credentials
        // — TODO: blind spot, low priority).
        if audit::should_audit_first_use(last_used.as_deref()) {
            audit::record_first_use(&self.db, user_id).await;
        }

        let author_name = login.clone();
        // TODO (blind spot B.16, Low): per-user override for primary email
        // when the user wants a real email rather than the GitHub no-reply.
        // Today we default to the no-reply form since we don't have a
        // captured primary email column yet (Phase 2a deferred it).
        let author_email = format!("{login}@users.noreply.github.com");
        Ok(GitToken {
            bearer: pat,
            source: TokenSource::UserPat,
            author_name: Some(author_name),
            author_email: Some(author_email),
            credential_row_id: row_id,
        })
    }
}

// ---------------------------------------------------------------------------
// Tests — integration
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{seal, MasterKey};

    fn in_mem_db_with_master_key(mk: MasterKey) -> Database {
        Database::open_in_memory()
            .expect("in-mem DB")
            .with_test_master_key(mk)
    }

    async fn seed_user(db: &Database, user_id: &str) {
        let conn = db.conn().lock().await;
        conn.execute(
            "INSERT INTO users (id, username, role) VALUES (?1, ?2, 'user')",
            rusqlite::params![user_id, user_id],
        )
        .unwrap();
    }

    async fn seed_pat(db: &Database, user_id: &str, sign_commits: bool) {
        let mk = db.master_key().expect("test mk").key.clone();
        let sealed = seal(&mk, b"ghp_test_pat").unwrap();
        let conn = db.conn().lock().await;
        github_credentials::upsert(
            &conn,
            user_id,
            &sealed,
            &format!("{user_id}-gh"),
            "[\"repo\"]",
            sign_commits,
        )
        .unwrap();
    }

    // ── DB-backed integration ─────────────────────────────────────────────

    #[tokio::test]
    async fn mode_for_user_app_plus_pat_when_both_present() {
        let db = in_mem_db_with_master_key(MasterKey::from_bytes([0xAA; 32]));
        seed_user(&db, "u-alice").await;
        seed_pat(&db, "u-alice", true).await;
        // App "present" — represented by a non-None Arc; the actual manager
        // isn't called in mode_for_user.
        let resolver = test_resolver_with_app(db);
        assert_eq!(
            resolver.mode_for_user("u-alice").await.unwrap(),
            GithubAuthMode::AppPlusPat
        );
    }

    #[tokio::test]
    async fn mode_for_user_app_only_when_app_and_no_pat() {
        let db = in_mem_db_with_master_key(MasterKey::from_bytes([0xBB; 32]));
        seed_user(&db, "u-alice").await;
        let resolver = test_resolver_with_app(db);
        assert_eq!(
            resolver.mode_for_user("u-alice").await.unwrap(),
            GithubAuthMode::AppOnly
        );
    }

    #[tokio::test]
    async fn mode_for_user_pat_only_when_pat_and_no_app() {
        let db = in_mem_db_with_master_key(MasterKey::from_bytes([0xCC; 32]));
        seed_user(&db, "u-alice").await;
        seed_pat(&db, "u-alice", true).await;
        let resolver = GitAuthResolver::new(db, None);
        assert_eq!(
            resolver.mode_for_user("u-alice").await.unwrap(),
            GithubAuthMode::PatOnly
        );
    }

    #[tokio::test]
    async fn mode_for_user_missing_when_neither() {
        let db = in_mem_db_with_master_key(MasterKey::from_bytes([0xDD; 32]));
        seed_user(&db, "u-alice").await;
        let resolver = GitAuthResolver::new(db, None);
        assert_eq!(
            resolver.mode_for_user("u-alice").await.unwrap(),
            GithubAuthMode::Missing
        );
    }

    #[tokio::test]
    async fn token_for_clone_with_pat_only_returns_user_pat_unsealed() {
        let db = in_mem_db_with_master_key(MasterKey::from_bytes([0x11; 32]));
        seed_user(&db, "u-alice").await;
        seed_pat(&db, "u-alice", true).await;
        let resolver = GitAuthResolver::new(db, None);

        let token = resolver
            .token_for(GitAction::Clone, "u-alice")
            .await
            .expect("token_for clone");
        assert_eq!(token.source, TokenSource::UserPat);
        assert_eq!(token.bearer.expose(), "ghp_test_pat");
        assert_eq!(token.author_name.as_deref(), Some("u-alice-gh"));
        assert_eq!(
            token.author_email.as_deref(),
            Some("u-alice-gh@users.noreply.github.com")
        );
    }

    #[tokio::test]
    async fn token_for_returns_unauthenticated_error_for_missing_mode() {
        let db = in_mem_db_with_master_key(MasterKey::from_bytes([0x22; 32]));
        seed_user(&db, "u-alice").await;
        let resolver = GitAuthResolver::new(db, None);

        let err = resolver
            .token_for(GitAction::Clone, "u-alice")
            .await
            .expect_err("must be unauthenticated");
        assert!(matches!(err, GitAuthError::UnauthenticatedGit { .. }));
        assert_eq!(err.code(), "unauthenticated_git");
    }

    #[tokio::test]
    async fn token_for_returns_master_key_unavailable_when_db_lacks_key() {
        // Open in-memory without injecting a master key.
        let db = Database::open_in_memory().unwrap();
        seed_user(&db, "u-alice").await;
        // We need a row to attempt unseal; insert a dummy sealed blob.
        // Master key is None, so `unseal_user_pat` short-circuits before
        // even reading the row — but we still need a PAT row so the
        // resolver thinks the user is in PatOnly mode.
        {
            let conn = db.conn().lock().await;
            conn.execute(
                "INSERT INTO user_github_credentials \
                 (user_id, ciphertext, nonce, wrapped_dek, wnonce, github_login, scopes_json, sign_commits) \
                 VALUES ('u-alice', X'00', randomblob(24), X'00', randomblob(24), 'alice', '[\"repo\"]', 1)",
                [],
            )
            .unwrap();
        }

        let resolver = GitAuthResolver::new(db, None);
        let err = resolver
            .token_for(GitAction::Clone, "u-alice")
            .await
            .expect_err("must be MasterKeyUnavailable");
        assert!(matches!(err, GitAuthError::MasterKeyUnavailable { .. }));
        assert_eq!(err.code(), "master_key_unavailable");
    }

    #[tokio::test]
    async fn token_for_audit_logs_used_on_first_use() {
        let db = in_mem_db_with_master_key(MasterKey::from_bytes([0x33; 32]));
        seed_user(&db, "u-alice").await;
        seed_pat(&db, "u-alice", true).await;
        let resolver = GitAuthResolver::new(db.clone(), None);

        // First use → audit row.
        let pre: i64 = {
            let conn = db.conn().lock().await;
            conn.query_row(
                "SELECT COUNT(*) FROM credential_audit WHERE event='used'",
                [],
                |r| r.get(0),
            )
            .unwrap()
        };
        let _ = resolver
            .token_for(GitAction::Clone, "u-alice")
            .await
            .unwrap();
        let post: i64 = {
            let conn = db.conn().lock().await;
            conn.query_row(
                "SELECT COUNT(*) FROM credential_audit WHERE event='used'",
                [],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(post, pre + 1, "first use must write an audit row");

        // Second use within 60s → debounced.
        let _ = resolver
            .token_for(GitAction::Clone, "u-alice")
            .await
            .unwrap();
        let post2: i64 = {
            let conn = db.conn().lock().await;
            conn.query_row(
                "SELECT COUNT(*) FROM credential_audit WHERE event='used'",
                [],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(
            post2, post,
            "second use within debounce window must NOT write another row"
        );
    }

    #[tokio::test]
    async fn revalidate_sso_no_pat_is_ok() {
        let db = in_mem_db_with_master_key(MasterKey::from_bytes([0x44; 32]));
        seed_user(&db, "u-alice").await;
        let resolver = GitAuthResolver::new(db, None);

        let mock = crate::auth::RealGhClient::new();
        // No PAT → resolver returns Ok without touching gh.
        resolver
            .revalidate_sso("u-alice", &mock, &["any-org".to_string()])
            .await
            .expect("no-pat path must succeed");
    }

    #[tokio::test]
    async fn revalidate_sso_returns_sso_required_when_pat_blocked() {
        let db = in_mem_db_with_master_key(MasterKey::from_bytes([0x55; 32]));
        seed_user(&db, "u-alice").await;
        seed_pat(&db, "u-alice", true).await;
        let resolver = GitAuthResolver::new(db, None);

        // Build a mock GhClient that returns "valid user" + SSO-blocked org.
        struct SsoBlockedGh;
        #[async_trait::async_trait]
        impl GhClient for SsoBlockedGh {
            async fn api_user(
                &self,
                _pat: &str,
            ) -> std::result::Result<crate::auth::GhResponse, String> {
                Ok(crate::auth::GhResponse {
                    status: 200,
                    headers: vec![("X-OAuth-Scopes".into(), "repo".into())],
                    body: "{\"login\":\"alice\"}".into(),
                })
            }
            async fn api_org(
                &self,
                _pat: &str,
                _org: &str,
            ) -> std::result::Result<crate::auth::GhResponse, String> {
                Ok(crate::auth::GhResponse {
                    status: 403,
                    headers: vec![(
                        "X-GitHub-SSO".into(),
                        "required; url=https://github.com/orgs/foo/sso?return_to=x".into(),
                    )],
                    body: "{}".into(),
                })
            }
        }

        let err = resolver
            .revalidate_sso("u-alice", &SsoBlockedGh, &["foo".to_string()])
            .await
            .expect_err("must require SSO");
        match err {
            GitAuthError::SsoAuthorizationRequired { org, sso_url } => {
                assert_eq!(org, "foo");
                assert!(sso_url.contains("orgs/foo/sso"));
            }
            other => panic!("expected SsoAuthorizationRequired; got {other:?}"),
        }
        assert_eq!(GitAuthError::SsoAuthorizationRequired {
            org: "x".into(),
            sso_url: "y".into(),
        }.code(), "sso_authorization_required");
    }

    /// Build a resolver with a fake App-token manager. The fake's
    /// `get_installation_token` is never called by `mode_for_user` or the
    /// pure decision function, so the encoding key doesn't matter — only
    /// `self.app.is_some()` does.
    fn test_resolver_with_app(db: Database) -> GitAuthResolver {
        let mgr = GitHubAppTokenManager::for_tests(1, 1);
        GitAuthResolver::new(db, Some(Arc::new(mgr)))
    }

    // ─── revalidate_pat_for_workflow (Phase 2b.3.x) ─────────────────────
    //
    // We need a `GhClient` mock here that the tests can swap between
    // happy and SSO-failure responses. The one in `pat_validation::tests`
    // isn't `pub(crate)`, so define a minimal local one. Hitting real
    // github.com from tests is forbidden by the OSS-hygiene policy.

    struct RevalMockGh {
        user: std::sync::Mutex<crate::auth::gh_client::GhResponse>,
        org: std::sync::Mutex<crate::auth::gh_client::GhResponse>,
    }

    impl RevalMockGh {
        fn ok(scopes: &str) -> Self {
            Self {
                user: std::sync::Mutex::new(crate::auth::gh_client::GhResponse {
                    status: 200,
                    headers: vec![("X-OAuth-Scopes".into(), scopes.into())],
                    body: "{\"login\":\"alice\"}".into(),
                }),
                org: std::sync::Mutex::new(crate::auth::gh_client::GhResponse {
                    status: 200,
                    headers: vec![],
                    body: "{}".into(),
                }),
            }
        }
        fn sso_blocked() -> Self {
            Self {
                user: std::sync::Mutex::new(crate::auth::gh_client::GhResponse {
                    status: 200,
                    headers: vec![("X-OAuth-Scopes".into(), "repo, read:org".into())],
                    body: "{\"login\":\"alice\"}".into(),
                }),
                org: std::sync::Mutex::new(crate::auth::gh_client::GhResponse {
                    status: 403,
                    headers: vec![(
                        "X-GitHub-SSO".into(),
                        "required; url=https://github.com/sso?org_id=42".into(),
                    )],
                    body: "{}".into(),
                }),
            }
        }
    }

    #[async_trait::async_trait]
    impl crate::auth::gh_client::GhClient for RevalMockGh {
        async fn api_user(
            &self,
            _pat: &str,
        ) -> Result<crate::auth::gh_client::GhResponse, String> {
            Ok(self.user.lock().unwrap().clone())
        }
        async fn api_org(
            &self,
            _pat: &str,
            _org: &str,
        ) -> Result<crate::auth::gh_client::GhResponse, String> {
            Ok(self.org.lock().unwrap().clone())
        }
    }

    #[tokio::test]
    async fn revalidate_pat_for_workflow_skips_when_user_has_no_pat() {
        let db = in_mem_db_with_master_key(MasterKey::from_bytes([0x70; 32]));
        seed_user(&db, "u-noah").await;
        let resolver = GitAuthResolver::new(db.clone(), None);
        let gh = RevalMockGh::ok("repo");
        // No PAT row → must return Ok(()) without touching gh.
        resolver
            .revalidate_pat_for_workflow("u-noah", &gh, &[])
            .await
            .expect("must skip silently when user has no PAT");
        // And must NOT write any audit row.
        let audit_count: i64 = {
            let conn = db.conn().lock().await;
            conn.query_row(
                "SELECT COUNT(*) FROM credential_audit WHERE user_id = 'u-noah'",
                [],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(audit_count, 0, "no-PAT path must not write audit rows");
    }

    #[tokio::test]
    async fn revalidate_pat_for_workflow_happy_path_returns_ok() {
        let db = in_mem_db_with_master_key(MasterKey::from_bytes([0x71; 32]));
        seed_user(&db, "u-alice").await;
        seed_pat(&db, "u-alice", true).await;
        let resolver = GitAuthResolver::new(db.clone(), None);
        let gh = RevalMockGh::ok("repo, read:org");
        resolver
            .revalidate_pat_for_workflow("u-alice", &gh, &[])
            .await
            .expect("PAT with full scopes must pass revalidation");
        // Success path must NOT log a validation_failed row.
        let failed_count: i64 = {
            let conn = db.conn().lock().await;
            conn.query_row(
                "SELECT COUNT(*) FROM credential_audit \
                 WHERE user_id = 'u-alice' AND event = 'validation_failed'",
                [],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(failed_count, 0);
    }

    #[tokio::test]
    async fn revalidate_pat_for_workflow_sso_failure_writes_audit_and_returns_typed_err() {
        let db = in_mem_db_with_master_key(MasterKey::from_bytes([0x72; 32]));
        seed_user(&db, "u-eve").await;
        seed_pat(&db, "u-eve", true).await;
        let resolver = GitAuthResolver::new(db.clone(), None);
        let gh = RevalMockGh::sso_blocked();
        // Pass the org so the org-check fires.
        let err = resolver
            .revalidate_pat_for_workflow("u-eve", &gh, &["acme".to_string()])
            .await
            .expect_err("SSO-blocked org must surface as Err");
        // Must be typed (not Internal).
        match err {
            GitAuthError::SsoAuthorizationRequired { ref org, .. } => {
                assert_eq!(org, "acme");
            }
            other => panic!("expected SsoAuthorizationRequired; got {other:?}"),
        }
        // And the failure must be recorded in credential_audit with the
        // mapped error code.
        let row: (String, Option<String>) = {
            let conn = db.conn().lock().await;
            conn.query_row(
                "SELECT event, error_code FROM credential_audit \
                 WHERE user_id = 'u-eve' AND event = 'validation_failed' \
                 ORDER BY at DESC LIMIT 1",
                [],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
            )
            .expect("must write a validation_failed audit row")
        };
        assert_eq!(row.0, "validation_failed");
        assert_eq!(row.1.as_deref(), Some("sso_authorization_required"));
    }

}
