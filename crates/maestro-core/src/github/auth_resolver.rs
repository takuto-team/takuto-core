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

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use crate::auth::{open, GhClient, SealedBlob};
use crate::db::credential_audit::{self, CredentialAuditKind};
use crate::db::github_credentials;
use crate::db::Database;
use crate::github_app::GitHubAppTokenManager;

// ---------------------------------------------------------------------------
// Action / token / source / mode enums
// ---------------------------------------------------------------------------

/// Categorises every git operation the engine performs so the resolver can
/// route to the right token source. The variants mirror the rows of the
/// decision matrix above.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitAction {
    /// Read-only fetch from the remote.
    Clone,
    /// Read-only fetch (subsequent updates of a repo we already cloned).
    Fetch,
    /// Writes commits to the remote. The branch on user-pat vs App is
    /// gated by the user's `attribute_commits` toggle (see arch A3).
    Push,
    /// `gh pr create` — opens a pull request.
    PullRequestCreate,
    /// Posts a comment on an existing pull request.
    PullRequestComment,
    /// Submits a review on an existing pull request.
    PullRequestReview,
    /// Comments on a GitHub issue.
    IssueComment,
    /// Webhook handler ingests payloads in the Maestro container — always
    /// uses the App identity since there's no user context.
    WebhookEventIngest,
}

impl GitAction {
    /// Stable identifier used in `tracing` lines.
    pub fn as_str(self) -> &'static str {
        match self {
            GitAction::Clone => "clone",
            GitAction::Fetch => "fetch",
            GitAction::Push => "push",
            GitAction::PullRequestCreate => "pull_request_create",
            GitAction::PullRequestComment => "pull_request_comment",
            GitAction::PullRequestReview => "pull_request_review",
            GitAction::IssueComment => "issue_comment",
            GitAction::WebhookEventIngest => "webhook_event_ingest",
        }
    }
}

/// The two token sources the resolver may return. Tests pattern-match on
/// this enum to check the decision matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenSource {
    /// GitHub App installation token (cached + auto-refreshed by
    /// [`GitHubAppTokenManager`]).
    App,
    /// User-supplied personal access token (sealed at rest in
    /// `user_github_credentials`).
    UserPat,
}

impl TokenSource {
    pub fn as_str(self) -> &'static str {
        match self {
            TokenSource::App => "app",
            TokenSource::UserPat => "user_pat",
        }
    }
}

/// Per-user GitHub auth mode. Mirrors the wire-format `github_mode` field
/// on `/api/auth/status` (Phase 0) so the UI can render the right hint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GithubAuthMode {
    /// `[github]` App is configured AND the user has a stored PAT.
    AppPlusPat,
    /// `[github]` App is configured; the user has no stored PAT.
    AppOnly,
    /// No App configured; the user has a stored PAT.
    PatOnly,
    /// Neither — every write action fails fast with
    /// [`GitAuthError::UnauthenticatedGit`].
    Missing,
}

impl fmt::Display for GithubAuthMode {
    /// Wire-format string the dashboard renders.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            // Matches the existing Phase 0 vocabulary in
            // `crates/maestro-core/src/docker_hooks.rs::GitHubStatus.mode`.
            GithubAuthMode::AppPlusPat | GithubAuthMode::AppOnly => "app",
            GithubAuthMode::PatOnly => "pat_required",
            GithubAuthMode::Missing => "missing",
        };
        f.write_str(s)
    }
}

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
    /// Phase 2b.3 will pin this onto `PersistedWorkflowRecord.auth_pin` so a
    /// restored workflow still resolves the same credential row even after
    /// a deployment-wide provider switch invalidated newer rows. Phase 2b.2
    /// only populates it; nothing consumes it yet.
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

    /// Phase 2b.3 calls this at workflow start to re-check SSO authorisation
    /// for every org the workflow will touch. Phase 2b.2 only exposes it;
    /// the driver invocation lands later.
    pub async fn revalidate_sso(
        &self,
        user_id: &str,
        gh: &dyn GhClient,
        orgs: &[String],
    ) -> GitAuthResult<()> {
        // No PAT → nothing to revalidate. Return Ok so callers don't have to
        // branch on mode; SSO only matters for PAT-bearing flows.
        if !self.user_has_pat(user_id).await? {
            return Ok(());
        }
        let pat = self.unseal_user_pat(user_id).await?;
        match crate::auth::validate_pat(gh, pat.expose(), orgs).await {
            Ok(_) => Ok(()),
            Err(crate::auth::PatValidationError::SsoAuthorizationRequired { org, sso_url }) => {
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

    // ── Internals ─────────────────────────────────────────────────────────

    async fn user_has_pat(&self, user_id: &str) -> GitAuthResult<bool> {
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

    async fn unseal_user_pat(&self, user_id: &str) -> GitAuthResult<SecretToken> {
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
        let should_audit = should_audit_first_use(last_used.as_deref());
        if should_audit {
            let now = chrono::Utc::now()
                .format("%Y-%m-%dT%H:%M:%SZ")
                .to_string();
            let conn = self.db.conn().lock().await;
            // touch_last_validated bumps the column we're using as the
            // debounce flag.
            let _ = github_credentials::touch_last_validated(&conn, user_id, &now);
            let _ = credential_audit::log(
                &conn,
                user_id,
                Some(user_id),
                CredentialAuditKind::GithubPat,
                None,
                "used",
                "ok",
                None,
            );
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

/// Pure decision function — does NOT contact the network or DB. Returns
/// `Some(source)` when the matrix has a verdict, `None` when no auth source
/// is available for the (action, mode) pair. The caller turns `None` into
/// [`GitAuthError::UnauthenticatedGit`].
pub fn decide_token_source(
    action: GitAction,
    mode: GithubAuthMode,
    attribute_commits: bool,
) -> GitAuthResult<Option<TokenSource>> {
    let source = match mode {
        GithubAuthMode::Missing => None,
        GithubAuthMode::AppOnly => Some(TokenSource::App),
        GithubAuthMode::PatOnly => Some(TokenSource::UserPat),
        GithubAuthMode::AppPlusPat => Some(match action {
            // Read-only + webhook → App (lower rate-limit cost, no user
            // attribution needed).
            GitAction::Clone | GitAction::Fetch | GitAction::WebhookEventIngest => TokenSource::App,
            // Push toggles on `attribute_commits` — true means we want the
            // commit to show as the user, so we use the user's PAT.
            GitAction::Push => {
                if attribute_commits {
                    TokenSource::UserPat
                } else {
                    TokenSource::App
                }
            }
            // Everything else is a write that should show as the user.
            GitAction::PullRequestCreate
            | GitAction::PullRequestComment
            | GitAction::PullRequestReview
            | GitAction::IssueComment => TokenSource::UserPat,
        }),
    };
    Ok(source)
}

/// "First use in the last minute" debounce. Returns `true` if we should
/// emit an audit row for this use. `last_used` is the previous
/// `last_validated_at` string we co-opt as a debounce flag.
fn should_audit_first_use(last_used: Option<&str>) -> bool {
    let Some(prev) = last_used else {
        return true;
    };
    // Anything we can't parse as RFC-3339 we audit (conservatively re-emit).
    let prev_dt = match chrono::DateTime::parse_from_rfc3339(prev) {
        Ok(dt) => dt.with_timezone(&chrono::Utc),
        Err(_) => return true,
    };
    chrono::Utc::now() - prev_dt > chrono::Duration::seconds(60)
}

// ---------------------------------------------------------------------------
// Tests — decision matrix (28 cells) + integration
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

    /// ── Pure decision function: 28 cells (7 actions × 4 modes) ──────────
    ///
    /// Each cell asserts:
    ///   (action, mode, attribute_commits) → expected_source
    ///
    /// Push is the only row that depends on `attribute_commits`, so we test
    /// each Push cell twice (true, false).

    #[test]
    fn matrix_clone_mode_app_plus_pat_picks_app() {
        let got = decide_token_source(GitAction::Clone, GithubAuthMode::AppPlusPat, true).unwrap();
        assert_eq!(got, Some(TokenSource::App));
    }
    #[test]
    fn matrix_clone_mode_app_only_picks_app() {
        let got = decide_token_source(GitAction::Clone, GithubAuthMode::AppOnly, true).unwrap();
        assert_eq!(got, Some(TokenSource::App));
    }
    #[test]
    fn matrix_clone_mode_pat_only_picks_user_pat() {
        let got = decide_token_source(GitAction::Clone, GithubAuthMode::PatOnly, true).unwrap();
        assert_eq!(got, Some(TokenSource::UserPat));
    }
    #[test]
    fn matrix_clone_mode_missing_is_unauthenticated() {
        let got = decide_token_source(GitAction::Clone, GithubAuthMode::Missing, true).unwrap();
        assert_eq!(got, None);
    }

    #[test]
    fn matrix_fetch_mode_app_plus_pat_picks_app() {
        let got = decide_token_source(GitAction::Fetch, GithubAuthMode::AppPlusPat, true).unwrap();
        assert_eq!(got, Some(TokenSource::App));
    }
    #[test]
    fn matrix_fetch_mode_app_only_picks_app() {
        let got = decide_token_source(GitAction::Fetch, GithubAuthMode::AppOnly, true).unwrap();
        assert_eq!(got, Some(TokenSource::App));
    }
    #[test]
    fn matrix_fetch_mode_pat_only_picks_user_pat() {
        let got = decide_token_source(GitAction::Fetch, GithubAuthMode::PatOnly, true).unwrap();
        assert_eq!(got, Some(TokenSource::UserPat));
    }
    #[test]
    fn matrix_fetch_mode_missing_is_unauthenticated() {
        let got = decide_token_source(GitAction::Fetch, GithubAuthMode::Missing, true).unwrap();
        assert_eq!(got, None);
    }

    #[test]
    fn matrix_push_attribute_true_mode_app_plus_pat_picks_user_pat() {
        let got = decide_token_source(GitAction::Push, GithubAuthMode::AppPlusPat, true).unwrap();
        assert_eq!(got, Some(TokenSource::UserPat));
    }
    #[test]
    fn matrix_push_attribute_false_mode_app_plus_pat_picks_app() {
        let got = decide_token_source(GitAction::Push, GithubAuthMode::AppPlusPat, false).unwrap();
        assert_eq!(got, Some(TokenSource::App));
    }
    #[test]
    fn matrix_push_mode_app_only_always_picks_app() {
        assert_eq!(
            decide_token_source(GitAction::Push, GithubAuthMode::AppOnly, true).unwrap(),
            Some(TokenSource::App)
        );
        assert_eq!(
            decide_token_source(GitAction::Push, GithubAuthMode::AppOnly, false).unwrap(),
            Some(TokenSource::App)
        );
    }
    #[test]
    fn matrix_push_mode_pat_only_always_picks_user_pat() {
        assert_eq!(
            decide_token_source(GitAction::Push, GithubAuthMode::PatOnly, true).unwrap(),
            Some(TokenSource::UserPat)
        );
        assert_eq!(
            decide_token_source(GitAction::Push, GithubAuthMode::PatOnly, false).unwrap(),
            Some(TokenSource::UserPat)
        );
    }
    #[test]
    fn matrix_push_mode_missing_is_unauthenticated_for_both_attribute_flags() {
        assert_eq!(
            decide_token_source(GitAction::Push, GithubAuthMode::Missing, true).unwrap(),
            None
        );
        assert_eq!(
            decide_token_source(GitAction::Push, GithubAuthMode::Missing, false).unwrap(),
            None
        );
    }

    #[test]
    fn matrix_pr_create_mode_app_plus_pat_picks_user_pat() {
        let got = decide_token_source(
            GitAction::PullRequestCreate,
            GithubAuthMode::AppPlusPat,
            true,
        )
        .unwrap();
        assert_eq!(got, Some(TokenSource::UserPat));
    }
    #[test]
    fn matrix_pr_create_mode_app_only_picks_app() {
        let got = decide_token_source(
            GitAction::PullRequestCreate,
            GithubAuthMode::AppOnly,
            true,
        )
        .unwrap();
        assert_eq!(got, Some(TokenSource::App));
    }
    #[test]
    fn matrix_pr_create_mode_pat_only_picks_user_pat() {
        let got = decide_token_source(
            GitAction::PullRequestCreate,
            GithubAuthMode::PatOnly,
            true,
        )
        .unwrap();
        assert_eq!(got, Some(TokenSource::UserPat));
    }
    #[test]
    fn matrix_pr_create_mode_missing_is_unauthenticated() {
        let got = decide_token_source(
            GitAction::PullRequestCreate,
            GithubAuthMode::Missing,
            true,
        )
        .unwrap();
        assert_eq!(got, None);
    }

    #[test]
    fn matrix_pr_comment_mode_app_plus_pat_picks_user_pat() {
        let got = decide_token_source(
            GitAction::PullRequestComment,
            GithubAuthMode::AppPlusPat,
            true,
        )
        .unwrap();
        assert_eq!(got, Some(TokenSource::UserPat));
    }
    #[test]
    fn matrix_pr_comment_mode_app_only_picks_app() {
        let got = decide_token_source(
            GitAction::PullRequestComment,
            GithubAuthMode::AppOnly,
            true,
        )
        .unwrap();
        assert_eq!(got, Some(TokenSource::App));
    }
    #[test]
    fn matrix_pr_comment_mode_pat_only_picks_user_pat() {
        let got = decide_token_source(
            GitAction::PullRequestComment,
            GithubAuthMode::PatOnly,
            true,
        )
        .unwrap();
        assert_eq!(got, Some(TokenSource::UserPat));
    }
    #[test]
    fn matrix_pr_comment_mode_missing_is_unauthenticated() {
        let got = decide_token_source(
            GitAction::PullRequestComment,
            GithubAuthMode::Missing,
            true,
        )
        .unwrap();
        assert_eq!(got, None);
    }

    #[test]
    fn matrix_pr_review_mode_app_plus_pat_picks_user_pat() {
        let got = decide_token_source(
            GitAction::PullRequestReview,
            GithubAuthMode::AppPlusPat,
            true,
        )
        .unwrap();
        assert_eq!(got, Some(TokenSource::UserPat));
    }
    #[test]
    fn matrix_pr_review_mode_app_only_picks_app() {
        let got = decide_token_source(
            GitAction::PullRequestReview,
            GithubAuthMode::AppOnly,
            true,
        )
        .unwrap();
        assert_eq!(got, Some(TokenSource::App));
    }
    #[test]
    fn matrix_pr_review_mode_pat_only_picks_user_pat() {
        let got = decide_token_source(
            GitAction::PullRequestReview,
            GithubAuthMode::PatOnly,
            true,
        )
        .unwrap();
        assert_eq!(got, Some(TokenSource::UserPat));
    }
    #[test]
    fn matrix_pr_review_mode_missing_is_unauthenticated() {
        let got = decide_token_source(
            GitAction::PullRequestReview,
            GithubAuthMode::Missing,
            true,
        )
        .unwrap();
        assert_eq!(got, None);
    }

    #[test]
    fn matrix_issue_comment_mode_app_plus_pat_picks_user_pat() {
        let got = decide_token_source(
            GitAction::IssueComment,
            GithubAuthMode::AppPlusPat,
            true,
        )
        .unwrap();
        assert_eq!(got, Some(TokenSource::UserPat));
    }
    #[test]
    fn matrix_issue_comment_mode_app_only_picks_app() {
        let got = decide_token_source(
            GitAction::IssueComment,
            GithubAuthMode::AppOnly,
            true,
        )
        .unwrap();
        assert_eq!(got, Some(TokenSource::App));
    }
    #[test]
    fn matrix_issue_comment_mode_pat_only_picks_user_pat() {
        let got = decide_token_source(
            GitAction::IssueComment,
            GithubAuthMode::PatOnly,
            true,
        )
        .unwrap();
        assert_eq!(got, Some(TokenSource::UserPat));
    }
    #[test]
    fn matrix_issue_comment_mode_missing_is_unauthenticated() {
        let got = decide_token_source(
            GitAction::IssueComment,
            GithubAuthMode::Missing,
            true,
        )
        .unwrap();
        assert_eq!(got, None);
    }

    #[test]
    fn matrix_webhook_mode_app_plus_pat_picks_app() {
        let got = decide_token_source(
            GitAction::WebhookEventIngest,
            GithubAuthMode::AppPlusPat,
            true,
        )
        .unwrap();
        assert_eq!(got, Some(TokenSource::App));
    }
    #[test]
    fn matrix_webhook_mode_app_only_picks_app() {
        let got = decide_token_source(
            GitAction::WebhookEventIngest,
            GithubAuthMode::AppOnly,
            true,
        )
        .unwrap();
        assert_eq!(got, Some(TokenSource::App));
    }
    #[test]
    fn matrix_webhook_mode_pat_only_picks_user_pat() {
        let got = decide_token_source(
            GitAction::WebhookEventIngest,
            GithubAuthMode::PatOnly,
            true,
        )
        .unwrap();
        assert_eq!(got, Some(TokenSource::UserPat));
    }
    #[test]
    fn matrix_webhook_mode_missing_is_unauthenticated() {
        let got = decide_token_source(
            GitAction::WebhookEventIngest,
            GithubAuthMode::Missing,
            true,
        )
        .unwrap();
        assert_eq!(got, None);
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

    #[test]
    fn github_auth_mode_display_matches_phase_0_wire_strings() {
        assert_eq!(GithubAuthMode::AppPlusPat.to_string(), "app");
        assert_eq!(GithubAuthMode::AppOnly.to_string(), "app");
        assert_eq!(GithubAuthMode::PatOnly.to_string(), "pat_required");
        assert_eq!(GithubAuthMode::Missing.to_string(), "missing");
    }

    #[test]
    fn secret_token_debug_does_not_leak_bytes() {
        let t = SecretToken::new("ghp_super_secret_token".into());
        let s = format!("{t:?}");
        assert!(!s.contains("ghp_super_secret_token"));
        assert!(s.contains("redacted"));
    }

    /// Build a resolver with a fake App-token manager. The fake's
    /// `get_installation_token` is never called by `mode_for_user` or the
    /// pure decision function, so the encoding key doesn't matter — only
    /// `self.app.is_some()` does.
    fn test_resolver_with_app(db: Database) -> GitAuthResolver {
        let mgr = GitHubAppTokenManager::for_tests(1, 1);
        GitAuthResolver::new(db, Some(Arc::new(mgr)))
    }
}
