// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! GitHub App authentication for bot-attributed commits and pull requests.
//!
//! When configured, Maestro uses a GitHub App installation token instead of the
//! personal `gh` CLI credentials. Commits and PRs are attributed to the App's
//! bot identity (e.g. `maestro-bot[bot]`).
//!
//! # Authentication flow
//!
//! 1. Generate a short-lived RS256 JWT from the App ID + private key.
//! 2. Exchange the JWT for an installation access token via the GitHub API.
//! 3. Configure `gh` and git in the worktree to use the installation token.
//! 4. Tokens are cached and refreshed 5 minutes before expiry.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{Duration, Utc};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::config::GitHubAppConfig;
use crate::error::Result;
use crate::process;

pub mod error;
pub use error::GitHubAppError;

/// Well-known path where the background task writes the current installation token.
/// Worker containers read this file on every `gh` / `git` invocation so they always
/// use a valid, non-expired token — even for steps that run longer than 1 hour.
///
/// Lives inside the `gh-auth` Docker volume, which is mounted at the same path in
/// both the main Maestro container and worker containers.
pub const TOKEN_FILE_PATH: &str = "/home/maestro/.config/gh/gh-app-token";

/// How often the background task checks the token expiry (seconds).
const TOKEN_REFRESH_POLL_SECS: u64 = 300; // 5 minutes

// ---------------------------------------------------------------------------
// JWT claims
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct JwtClaims {
    /// Issuer — the GitHub App ID (as a string per GitHub docs).
    iss: String,
    /// Issued-at (Unix timestamp, backdated 60 s for clock skew).
    iat: i64,
    /// Expiration (Unix timestamp, max 10 minutes from now).
    exp: i64,
}

// ---------------------------------------------------------------------------
// GitHub API response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct InstallationTokenResponse {
    token: String,
    expires_at: String,
}

#[derive(Debug, Deserialize)]
struct GitHubApiError {
    message: String,
    #[serde(default)]
    documentation_url: String,
}

// ---------------------------------------------------------------------------
// Cached token
// ---------------------------------------------------------------------------

struct CachedInstallationToken {
    token: String,
    expires_at: chrono::DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Token manager
// ---------------------------------------------------------------------------

/// Manages GitHub App JWT generation and installation access token caching.
///
/// Thread-safe: the internal cache uses `tokio::sync::RwLock` so multiple
/// workflows can share one manager without contention.
pub struct GitHubAppTokenManager {
    app_id: u64,
    installation_id: u64,
    encoding_key: EncodingKey,
    cached: RwLock<Option<CachedInstallationToken>>,
}

impl std::fmt::Debug for GitHubAppTokenManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitHubAppTokenManager")
            .field("app_id", &self.app_id)
            .field("installation_id", &self.installation_id)
            .finish_non_exhaustive()
    }
}

impl GitHubAppTokenManager {
    /// Create a new token manager from the config.
    ///
    /// Parses the PEM private key eagerly. Returns `Err` with an actionable
    /// message when the key is malformed or missing.
    pub fn new(config: &GitHubAppConfig) -> Result<Self> {
        let key_pem = Self::resolve_private_key(config)?;
        let encoding_key = EncodingKey::from_rsa_pem(key_pem.as_bytes())
            .map_err(|source| GitHubAppError::InvalidPrivateKey { source })?;

        Ok(Self {
            app_id: config.app_id,
            installation_id: config.app_installation_id,
            encoding_key,
            cached: RwLock::new(None),
        })
    }

    /// **Test-only** constructor used by sibling crates in the workspace
    /// that need a `GitHubAppTokenManager` instance for mode-checks (where
    /// `.is_some()` is the only thing that matters and
    /// `get_installation_token` is never expected to succeed). The encoding
    /// key is a meaningless secret — any call to `fetch_installation_token`
    /// will fail at the JWT signing step. Production callers must use
    /// [`Self::new`].
    ///
    /// Marked `#[doc(hidden)]` to keep it out of the rendered docs and
    /// signal at the API surface that it's not for production use.
    #[doc(hidden)]
    pub fn for_tests(app_id: u64, installation_id: u64) -> Self {
        Self {
            app_id,
            installation_id,
            encoding_key: EncodingKey::from_secret(b"test-only-not-a-real-key"),
            cached: RwLock::new(None),
        }
    }

    fn resolve_private_key(config: &GitHubAppConfig) -> Result<String> {
        let has_inline = !config.app_private_key.trim().is_empty();
        let has_path = !config.app_private_key_path.trim().is_empty();

        if has_inline && has_path {
            return Err(GitHubAppError::PrivateKeyConfigConflict.into());
        }

        if has_inline {
            return Ok(config.app_private_key.clone());
        }

        if has_path {
            let path = config.app_private_key_path.trim();
            return std::fs::read_to_string(path).map_err(|source| {
                GitHubAppError::PrivateKeyRead {
                    path: PathBuf::from(path),
                    source,
                }
                .into()
            });
        }

        Err(GitHubAppError::PrivateKeyMissing.into())
    }

    // -- JWT --

    fn generate_jwt(&self) -> Result<String> {
        let now = Utc::now();
        let claims = JwtClaims {
            iss: self.app_id.to_string(),
            // Backdate 60 s for clock skew (GitHub recommendation).
            iat: (now - Duration::seconds(60)).timestamp(),
            // GitHub caps at 10 minutes; use 9 min 30 s for safety.
            exp: (now + Duration::seconds(570)).timestamp(),
        };

        let header = Header::new(Algorithm::RS256);
        encode(&header, &claims, &self.encoding_key)
            .map_err(|source| GitHubAppError::JwtSigning { source }.into())
    }

    // -- Installation token --

    /// Return a valid installation access token, fetching or refreshing as needed.
    pub async fn get_installation_token(&self, cwd: &Path) -> Result<String> {
        // Fast path: read lock.
        {
            let cached = self.cached.read().await;
            if let Some(ref ct) = *cached
                && ct.expires_at > Utc::now() + Duration::minutes(5)
            {
                return Ok(ct.token.clone());
            }
        }

        // Slow path: write lock + refresh.
        let mut cached = self.cached.write().await;
        // Re-check after acquiring write lock (another task may have refreshed).
        if let Some(ref ct) = *cached
            && ct.expires_at > Utc::now() + Duration::minutes(5)
        {
            return Ok(ct.token.clone());
        }

        let (token, expires_at) = self.fetch_installation_token(cwd).await?;
        *cached = Some(CachedInstallationToken {
            token: token.clone(),
            expires_at,
        });

        Ok(token)
    }

    async fn fetch_installation_token(
        &self,
        cwd: &Path,
    ) -> Result<(String, chrono::DateTime<Utc>)> {
        let jwt = self.generate_jwt()?;

        let auth_header = format!("Authorization: Bearer {jwt}");
        let url = format!(
            "https://api.github.com/app/installations/{}/access_tokens",
            self.installation_id
        );

        let output = process::run_command(
            "curl",
            &[
                "-s",
                "--max-time",
                "30",
                "--connect-timeout",
                "10",
                "-X",
                "POST",
                "-H",
                &auth_header,
                "-H",
                "Accept: application/vnd.github+json",
                "-H",
                "X-GitHub-Api-Version: 2022-11-28",
                &url,
            ],
            cwd,
            CancellationToken::new(),
        )
        .await?;

        if !output.success() {
            return Err(GitHubAppError::HttpRequestFailed {
                exit_code: output.exit_code,
                stderr: output.stderr.trim().to_string(),
            }
            .into());
        }

        // Try to parse as a successful token response.
        if let Ok(resp) = serde_json::from_str::<InstallationTokenResponse>(&output.stdout) {
            let expires_at = chrono::DateTime::parse_from_rfc3339(&resp.expires_at)
                .map_err(|source| GitHubAppError::ExpiresAtParse {
                    raw: resp.expires_at.clone(),
                    source,
                })?
                .with_timezone(&Utc);

            info!(
                app_id = self.app_id,
                installation_id = self.installation_id,
                expires_at = %expires_at,
                "GitHub App installation token obtained"
            );

            return Ok((resp.token, expires_at));
        }

        // Parse as a GitHub API error for an actionable message.
        if let Ok(api_err) = serde_json::from_str::<GitHubApiError>(&output.stdout) {
            return Err(self.format_api_error(&api_err).into());
        }

        Err(GitHubAppError::UnexpectedApiResponse {
            body: output.stdout.trim().to_string(),
        }
        .into())
    }

    /// Classify a structured GitHub API error response into the matching
    /// typed `GitHubAppError` variant. Returns a typed variant rather than
    /// a `String` so the source-chain Display walks via `Error::source()` and
    /// the four lock-in tests assert against `Display` substrings only.
    fn format_api_error(&self, err: &GitHubApiError) -> GitHubAppError {
        let msg = err.message.clone();
        let documentation_url = err.documentation_url.clone();

        if msg.contains("Not Found") || msg.contains("not found") {
            GitHubAppError::ApiInstallationNotFound {
                installation_id: self.installation_id,
                documentation_url,
            }
        } else if msg.contains("could not be decoded")
            || msg.contains("Unauthorized")
            || msg.contains("Bad credentials")
        {
            GitHubAppError::ApiJwtRejected {
                app_id: self.app_id,
                message: msg,
                documentation_url,
            }
        } else if msg.contains("Resource not accessible") || msg.contains("permission") {
            GitHubAppError::ApiPermissionDenied {
                message: msg,
                documentation_url,
            }
        } else {
            GitHubAppError::ApiOther {
                message: msg,
                documentation_url,
            }
        }
    }

    // -- Bot identity --

    /// Git author name for the bot (e.g. `maestro-bot[bot]`).
    pub fn bot_name(&self) -> &str {
        "maestro-bot[bot]"
    }

    /// Git author email for the bot (e.g. `123456+maestro-bot[bot]@users.noreply.github.com`).
    pub fn bot_email(&self) -> String {
        format!("{}+maestro-bot[bot]@users.noreply.github.com", self.app_id)
    }

    // -- Git / gh configuration --

    /// Configure git identity in `cwd` for the GitHub App bot and set up the
    /// `gh` credential helper.
    ///
    /// **Does not touch `hosts.yml`** — the installation token is injected as the
    /// `GH_TOKEN` environment variable into worker-container `docker run` invocations
    /// by `ContainerRunner` instead. This keeps the shared auth volume clean and the
    /// main container's personal gh user active for dashboard API calls.
    ///
    /// 1. Configures git credential helper via `gh auth setup-git` (writes local
    ///    `~/.gitconfig` only, not the shared gh config volume).
    /// 2. Sets `user.name` and `user.email` to the bot identity.
    pub async fn configure_git_and_gh_auth(
        &self,
        cwd: &Path,
        cancel: CancellationToken,
    ) -> Result<()> {
        // Install a git credential helper that reads the token from the shared file
        // written by the background token service. Falls back to $GH_TOKEN env var
        // for local development without the token file.
        // We do NOT use `gh auth setup-git` because it requires an active `gh` session
        // (gh auth login), which is intentionally skipped when a GitHub App is configured.
        let helper = format!(
            "!f() {{ echo protocol=https; echo host=github.com; echo username=x-access-token; \
             echo \"password=$(cat {TOKEN_FILE_PATH} 2>/dev/null || echo $GH_TOKEN)\"; }}; f"
        );
        let helper = helper.as_str();
        let cred_out = crate::process::run_command(
            "git",
            &[
                "config",
                "--global",
                "credential.https://github.com.helper",
                helper,
            ],
            cwd,
            cancel.child_token(),
        )
        .await?;
        if !cred_out.success() {
            warn!(
                stderr = %cred_out.stderr.trim(),
                "git config credential.helper failed — git fetch/push may not authenticate"
            );
        }

        // Set git identity to bot.
        let bot_name = self.bot_name();
        let bot_email = self.bot_email();

        let name_out = process::run_command(
            "git",
            &["config", "user.name", bot_name],
            cwd,
            cancel.child_token(),
        )
        .await?;
        if !name_out.success() {
            return Err(GitHubAppError::GitConfigFailed {
                setting: "user.name",
                stderr: name_out.stderr.trim().to_string(),
            }
            .into());
        }

        let email_out = process::run_command(
            "git",
            &["config", "user.email", &bot_email],
            cwd,
            cancel.child_token(),
        )
        .await?;
        if !email_out.success() {
            return Err(GitHubAppError::GitConfigFailed {
                setting: "user.email",
                stderr: email_out.stderr.trim().to_string(),
            }
            .into());
        }

        info!(
            git_name = %bot_name,
            git_email = %bot_email,
            app_id = self.app_id,
            "Git author configured for GitHub App bot identity (GH_TOKEN injected into worker containers)"
        );

        Ok(())
    }

    /// Return a fresh installation access token for injection into worker containers
    /// as the `GH_TOKEN` environment variable.
    ///
    /// Uses the internal cache; fetches a new token from GitHub only when the cached
    /// one is within 5 minutes of expiry.
    pub async fn get_token_for_injection(&self, cwd: &Path) -> Result<String> {
        self.get_installation_token(cwd).await
    }

    /// Spawn a background task that proactively refreshes the GitHub App installation
    /// token and writes it atomically to [`TOKEN_FILE_PATH`].
    ///
    /// Worker containers read this file via a `gh` wrapper and git credential helper
    /// instead of relying on a frozen `GH_TOKEN` env var set at `docker run` time.
    /// This ensures tokens stay fresh even for workflows that run longer than the
    /// 1-hour GitHub App token lifetime.
    ///
    /// The task runs until `cancel` is triggered (i.e. Maestro shutdown).
    pub fn start_token_file_writer(self: &Arc<Self>, cwd: PathBuf, cancel: CancellationToken) {
        let mgr = Arc::clone(self);
        let token_path = PathBuf::from(TOKEN_FILE_PATH);
        tokio::spawn(async move {
            info!(
                path = %token_path.display(),
                poll_secs = TOKEN_REFRESH_POLL_SECS,
                "GitHub App token file writer started"
            );

            // Initial write — fetch immediately so workers have a token from the start.
            if let Err(e) = refresh_and_write(&mgr, &cwd, &token_path).await {
                warn!(error = ?e, "Initial GitHub App token write failed; will retry");
            }

            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        info!("GitHub App token file writer shutting down");
                        break;
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_secs(TOKEN_REFRESH_POLL_SECS)) => {
                        if let Err(e) = refresh_and_write(&mgr, &cwd, &token_path).await {
                            error!(error = ?e, "GitHub App token refresh failed; workers may use a stale token");
                        }
                    }
                }
            }
        });
    }
}

/// Fetch a (possibly cached) token and write it atomically to `path`.
async fn refresh_and_write(mgr: &GitHubAppTokenManager, cwd: &Path, path: &Path) -> Result<()> {
    let token = mgr.get_installation_token(cwd).await?;
    write_token_file(path, &token)?;
    Ok(())
}

/// Atomic write: write to a temp sibling, then rename.
pub fn write_token_file(path: &Path, token: &str) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, token).map_err(|source| GitHubAppError::TokenFileWrite {
        path: tmp.clone(),
        source,
    })?;
    std::fs::rename(&tmp, path).map_err(|source| GitHubAppError::TokenFileRename {
        from: tmp.clone(),
        to: path.to_path_buf(),
        source,
    })?;
    info!(path = %path.display(), "GitHub App token file updated");
    Ok(())
}

/// Try to create a [`GitHubAppTokenManager`] from config.
///
/// Returns `None` (with a warning log) when:
/// - The `[github]` section is not configured (silent).
/// - The configuration is present but invalid (warning logged).
///
/// This ensures GitHub App errors are **non-fatal at startup**.
pub fn try_create_token_manager(config: &GitHubAppConfig) -> Option<Arc<GitHubAppTokenManager>> {
    if !config.is_configured() {
        return None;
    }

    match GitHubAppTokenManager::new(config) {
        Ok(mgr) => {
            info!(
                app_id = config.app_id,
                installation_id = config.app_installation_id,
                "GitHub App authentication configured — commits and PRs will use bot identity"
            );
            Some(Arc::new(mgr))
        }
        Err(e) => {
            warn!(
                error = ?e,
                "GitHub App configuration present but invalid — falling back to personal gh auth. \
                 Fix the [github] section in config.toml to enable bot-attributed commits and PRs."
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GitHubAppConfig;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn mgr_with_app_id(app_id: u64) -> GitHubAppTokenManager {
        GitHubAppTokenManager {
            app_id,
            installation_id: 12345,
            // EncodingKey does not expose a public constructor for testing without
            // a real key, so we only test the pure helper methods that don't call
            // generate_jwt / fetch_installation_token.
            encoding_key: EncodingKey::from_secret(b"dummy"),
            cached: RwLock::new(None),
        }
    }

    // -- resolve_private_key --

    #[test]
    fn resolve_private_key_inline() {
        let cfg = GitHubAppConfig {
            app_id: 1,
            app_installation_id: 1,
            app_private_key: "inline-pem-content".into(),
            ..Default::default()
        };
        assert_eq!(
            GitHubAppTokenManager::resolve_private_key(&cfg).unwrap(),
            "inline-pem-content"
        );
    }

    #[test]
    fn resolve_private_key_from_file() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(b"file-pem-content").unwrap();
        let cfg = GitHubAppConfig {
            app_id: 1,
            app_installation_id: 1,
            app_private_key_path: f.path().to_str().unwrap().to_string(),
            ..Default::default()
        };
        assert_eq!(
            GitHubAppTokenManager::resolve_private_key(&cfg).unwrap(),
            "file-pem-content"
        );
    }

    #[test]
    fn resolve_private_key_both_set_is_error() {
        let cfg = GitHubAppConfig {
            app_id: 1,
            app_installation_id: 1,
            app_private_key: "inline".into(),
            app_private_key_path: "/some/path".into(),
            ..Default::default()
        };
        assert!(GitHubAppTokenManager::resolve_private_key(&cfg).is_err());
    }

    #[test]
    fn resolve_private_key_neither_set_is_error() {
        let cfg = GitHubAppConfig::default();
        assert!(GitHubAppTokenManager::resolve_private_key(&cfg).is_err());
    }

    #[test]
    fn resolve_private_key_missing_file_is_error() {
        let cfg = GitHubAppConfig {
            app_id: 1,
            app_installation_id: 1,
            app_private_key_path: "/nonexistent/key.pem".into(),
            ..Default::default()
        };
        assert!(GitHubAppTokenManager::resolve_private_key(&cfg).is_err());
    }

    // -- bot_name / bot_email --

    #[test]
    fn bot_name_is_fixed() {
        assert_eq!(mgr_with_app_id(0).bot_name(), "maestro-bot[bot]");
    }

    #[test]
    fn bot_email_contains_app_id() {
        let mgr = mgr_with_app_id(123456);
        assert_eq!(
            mgr.bot_email(),
            "123456+maestro-bot[bot]@users.noreply.github.com"
        );
    }

    // -- format_api_error --

    #[test]
    fn format_api_error_not_found() {
        let mgr = mgr_with_app_id(1);
        let err = GitHubApiError {
            message: "Not Found".into(),
            documentation_url: String::new(),
        };
        let typed = mgr.format_api_error(&err);
        assert!(matches!(
            typed,
            GitHubAppError::ApiInstallationNotFound { .. }
        ));
        let msg = typed.to_string();
        assert!(msg.contains("installation not found"));
        assert!(msg.contains("app_installation_id"));
    }

    #[test]
    fn format_api_error_unauthorized() {
        let mgr = mgr_with_app_id(42);
        let err = GitHubApiError {
            message: "could not be decoded".into(),
            documentation_url: "https://docs.github.com/".into(),
        };
        let typed = mgr.format_api_error(&err);
        // `documentation_url` is now carried as a typed field (not inlined in
        // Display); assert directly on the variant rather than the message.
        match &typed {
            GitHubAppError::ApiJwtRejected {
                documentation_url, ..
            } => assert_eq!(documentation_url, "https://docs.github.com/"),
            other => panic!("expected ApiJwtRejected, got {other:?}"),
        }
        let msg = typed.to_string();
        assert!(msg.contains("JWT authentication failed"));
        assert!(msg.contains("42")); // app_id
    }

    #[test]
    fn format_api_error_permissions() {
        let mgr = mgr_with_app_id(1);
        let err = GitHubApiError {
            message: "Resource not accessible by integration".into(),
            documentation_url: String::new(),
        };
        let typed = mgr.format_api_error(&err);
        assert!(matches!(typed, GitHubAppError::ApiPermissionDenied { .. }));
        let msg = typed.to_string();
        assert!(msg.contains("lacks required permissions"));
        assert!(msg.contains("pull_requests"));
    }

    #[test]
    fn format_api_error_generic() {
        let mgr = mgr_with_app_id(1);
        let err = GitHubApiError {
            message: "Something unexpected".into(),
            documentation_url: String::new(),
        };
        let typed = mgr.format_api_error(&err);
        assert!(matches!(typed, GitHubAppError::ApiOther { .. }));
        let msg = typed.to_string();
        assert!(msg.contains("Something unexpected"));
    }

    // -- write_token_file --

    #[test]
    fn write_token_file_creates_and_is_readable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gh-app-token");
        super::write_token_file(&path, "ghs_test_token_abc").unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "ghs_test_token_abc");
    }

    #[test]
    fn write_token_file_overwrites_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gh-app-token");
        super::write_token_file(&path, "first").unwrap();
        super::write_token_file(&path, "second").unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents, "second");
        // Temp file must not linger
        assert!(!dir.path().join("gh-app-token.tmp").exists());
    }
}
