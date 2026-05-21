// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Git remote and GitHub App credentials.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitConfig {
    #[serde(default = "default_base_branch")]
    pub base_branch: String,
    /// Git remote name for fetch, worktree base ref, and push (default `origin`).
    #[serde(default = "default_git_remote")]
    pub remote: String,
    #[serde(default = "default_repo_path")]
    pub repo_path: String,
}

/// GitHub App credentials for bot-attributed commits and pull requests.
///
/// When all required fields are set, Maestro authenticates as the GitHub App's
/// bot identity instead of the personal `gh` user. Commits and PRs will be
/// attributed to `maestro-bot[bot]`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GitHubAppConfig {
    /// The GitHub App's numeric App ID.
    #[serde(default)]
    pub app_id: u64,
    /// The installation ID for the target org/repository.
    #[serde(default)]
    pub app_installation_id: u64,
    /// Display name of the GitHub App (e.g. `"sous-coder"`). Shown in the dashboard header.
    #[serde(default)]
    pub app_name: String,
    /// PEM-encoded RSA private key for signing JWTs (inline content).
    /// Set **either** this or `app_private_key_path`, not both.
    #[serde(default)]
    pub app_private_key: String,
    /// Path to a PEM-encoded RSA private key file.
    /// Set **either** this or `app_private_key`, not both.
    #[serde(default)]
    pub app_private_key_path: String,
}

impl GitHubAppConfig {
    /// Returns `true` when the minimum required fields are set (app_id, installation_id,
    /// and at least one private key source).
    pub fn is_configured(&self) -> bool {
        self.app_id != 0
            && self.app_installation_id != 0
            && (!self.app_private_key.trim().is_empty()
                || !self.app_private_key_path.trim().is_empty())
    }
}

fn default_base_branch() -> String {
    "main".to_string()
}
fn default_repo_path() -> String {
    "/workspace".to_string()
}
fn default_git_remote() -> String {
    "origin".to_string()
}

impl Default for GitConfig {
    fn default() -> Self {
        Self {
            base_branch: default_base_branch(),
            remote: default_git_remote(),
            repo_path: default_repo_path(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_app_config_unconfigured_by_default() {
        assert!(!GitHubAppConfig::default().is_configured());
    }

    #[test]
    fn github_app_config_requires_app_id() {
        let cfg = GitHubAppConfig {
            app_id: 0,
            app_installation_id: 42,
            app_private_key: "pem".into(),
            ..Default::default()
        };
        assert!(!cfg.is_configured());
    }

    #[test]
    fn github_app_config_requires_installation_id() {
        let cfg = GitHubAppConfig {
            app_id: 99,
            app_installation_id: 0,
            app_private_key: "pem".into(),
            ..Default::default()
        };
        assert!(!cfg.is_configured());
    }

    #[test]
    fn github_app_config_requires_private_key_source() {
        let cfg = GitHubAppConfig {
            app_id: 99,
            app_installation_id: 42,
            app_private_key: "   ".into(),
            app_private_key_path: "   ".into(),
            ..Default::default()
        };
        assert!(!cfg.is_configured());
    }

    #[test]
    fn github_app_config_configured_with_inline_key() {
        let cfg = GitHubAppConfig {
            app_id: 99,
            app_installation_id: 42,
            app_private_key: "-----BEGIN RSA PRIVATE KEY-----".into(),
            ..Default::default()
        };
        assert!(cfg.is_configured());
    }

    #[test]
    fn github_app_config_configured_with_key_path() {
        let cfg = GitHubAppConfig {
            app_id: 99,
            app_installation_id: 42,
            app_private_key_path: "/etc/maestro/key.pem".into(),
            ..Default::default()
        };
        assert!(cfg.is_configured());
    }
}
