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

use std::path::Path;
use std::sync::Arc;

use chrono::{Duration, Utc};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::config::GitHubAppConfig;
use crate::error::{MaestroError, Result};
use crate::process;

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
        let encoding_key = EncodingKey::from_rsa_pem(key_pem.as_bytes()).map_err(|e| {
            MaestroError::GitHubApp(format!(
                "Invalid RSA private key in [github] config: {e}. \
                 Ensure app_private_key contains a valid PEM-encoded RSA private key \
                 (or app_private_key_path points to one). The key should begin with \
                 '-----BEGIN RSA PRIVATE KEY-----'."
            ))
        })?;

        Ok(Self {
            app_id: config.app_id,
            installation_id: config.app_installation_id,
            encoding_key,
            cached: RwLock::new(None),
        })
    }

    fn resolve_private_key(config: &GitHubAppConfig) -> Result<String> {
        let has_inline = !config.app_private_key.trim().is_empty();
        let has_path = !config.app_private_key_path.trim().is_empty();

        if has_inline && has_path {
            return Err(MaestroError::GitHubApp(
                "Set either [github] app_private_key or app_private_key_path, not both".into(),
            ));
        }

        if has_inline {
            return Ok(config.app_private_key.clone());
        }

        if has_path {
            let path = config.app_private_key_path.trim();
            return std::fs::read_to_string(path).map_err(|e| {
                MaestroError::GitHubApp(format!(
                    "Cannot read [github] app_private_key_path '{path}': {e}. \
                     Verify the file exists and is readable."
                ))
            });
        }

        Err(MaestroError::GitHubApp(
            "GitHub App private key not configured. Set [github] app_private_key (PEM content) \
             or app_private_key_path (path to PEM file)."
                .into(),
        ))
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
            .map_err(|e| MaestroError::GitHubApp(format!("Failed to generate JWT: {e}")))
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
            return Err(MaestroError::GitHubApp(format!(
                "curl request to GitHub API failed (exit {}): {}",
                output.exit_code,
                output.stderr.trim()
            )));
        }

        // Try to parse as a successful token response.
        if let Ok(resp) = serde_json::from_str::<InstallationTokenResponse>(&output.stdout) {
            let expires_at = chrono::DateTime::parse_from_rfc3339(&resp.expires_at)
                .map_err(|e| {
                    MaestroError::GitHubApp(format!(
                        "Failed to parse token expiry '{0}': {e}",
                        resp.expires_at
                    ))
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
            return Err(MaestroError::GitHubApp(self.format_api_error(&api_err)));
        }

        Err(MaestroError::GitHubApp(format!(
            "Unexpected GitHub API response: {}",
            output.stdout.trim()
        )))
    }

    fn format_api_error(&self, err: &GitHubApiError) -> String {
        let msg = &err.message;
        let doc = if err.documentation_url.is_empty() {
            String::new()
        } else {
            format!(" See {}", err.documentation_url)
        };

        if msg.contains("Not Found") || msg.contains("not found") {
            format!(
                "GitHub App installation not found (installation_id = {}). \
                 Verify [github] app_installation_id is correct and the App is installed \
                 on your org/repo. Find installation IDs at \
                 https://github.com/settings/installations{doc}",
                self.installation_id
            )
        } else if msg.contains("could not be decoded")
            || msg.contains("Unauthorized")
            || msg.contains("Bad credentials")
        {
            format!(
                "GitHub App JWT authentication failed: {msg}. \
                 Check [github] app_id ({}) and ensure the private key matches this App.{doc}",
                self.app_id
            )
        } else if msg.contains("Resource not accessible") || msg.contains("permission") {
            format!(
                "GitHub App lacks required permissions: {msg}. \
                 The App needs at minimum: contents (write), pull_requests (write), \
                 metadata (read). Update permissions at \
                 https://github.com/settings/apps{doc}",
            )
        } else {
            format!("GitHub API error: {msg}{doc}")
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
        // Configure git to use gh as credential helper (for git push).
        // gh reads GH_TOKEN from the environment when present, so no login call is needed.
        let setup_output =
            process::run_command("gh", &["auth", "setup-git"], cwd, cancel.child_token()).await?;
        if !setup_output.success() {
            warn!(
                stderr = %setup_output.stderr.trim(),
                "gh auth setup-git returned non-zero (credential helper may not be fully configured)"
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
            return Err(MaestroError::GitHubApp(format!(
                "git config user.name failed: {}",
                name_out.stderr.trim()
            )));
        }

        let email_out = process::run_command(
            "git",
            &["config", "user.email", &bot_email],
            cwd,
            cancel.child_token(),
        )
        .await?;
        if !email_out.success() {
            return Err(MaestroError::GitHubApp(format!(
                "git config user.email failed: {}",
                email_out.stderr.trim()
            )));
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
            app_private_key_path: String::new(),
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
            app_private_key: String::new(),
            app_private_key_path: f.path().to_str().unwrap().to_string(),
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
            app_private_key: String::new(),
            app_private_key_path: "/nonexistent/key.pem".into(),
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
        let msg = mgr.format_api_error(&err);
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
        let msg = mgr.format_api_error(&err);
        assert!(msg.contains("JWT authentication failed"));
        assert!(msg.contains("42")); // app_id
        assert!(msg.contains("https://docs.github.com/"));
    }

    #[test]
    fn format_api_error_permissions() {
        let mgr = mgr_with_app_id(1);
        let err = GitHubApiError {
            message: "Resource not accessible by integration".into(),
            documentation_url: String::new(),
        };
        let msg = mgr.format_api_error(&err);
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
        let msg = mgr.format_api_error(&err);
        assert!(msg.contains("Something unexpected"));
    }
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
                error = %e,
                "GitHub App configuration present but invalid — falling back to personal gh auth. \
                 Fix the [github] section in config.toml to enable bot-attributed commits and PRs."
            );
            None
        }
    }
}
