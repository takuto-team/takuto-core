// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.
#![allow(deprecated)] // Transitional: ConfigStr sites rewritten to ConfigError variants in C2.

use std::path::Path;

use crate::error::{MaestroError, Result};

/// Resolve the remote URL from a git repository by running `git remote get-url <remote>`.
///
/// Returns the remote URL string (trimmed). Errors if the path is not a git repo
/// or the remote does not exist.
pub async fn resolve_remote_url(repo_path: &Path, remote: &str) -> Result<String> {
    let output = crate::process::run_command(
        "git",
        &["remote", "get-url", remote],
        repo_path,
        tokio_util::sync::CancellationToken::new(),
    )
    .await?;

    if !output.success() {
        return Err(MaestroError::ConfigStr(format!(
            "Failed to resolve git remote URL for '{}' in {}: {}",
            remote,
            repo_path.display(),
            output.stderr.trim()
        )));
    }

    let url = output.stdout.trim().to_string();
    if url.is_empty() {
        return Err(MaestroError::ConfigStr(format!(
            "Git remote '{}' in {} returned an empty URL",
            remote,
            repo_path.display()
        )));
    }

    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    fn init_repo_with_remote(remote_name: &str, remote_url: &str) -> TempDir {
        let dir = TempDir::new().unwrap();
        Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["remote", "add", remote_name, remote_url])
            .current_dir(dir.path())
            .output()
            .unwrap();
        dir
    }

    #[tokio::test]
    async fn resolve_remote_url_returns_url_for_valid_repo() {
        let dir = init_repo_with_remote("origin", "https://github.com/owner/repo.git");
        let result = resolve_remote_url(dir.path(), "origin").await;
        assert_eq!(result.unwrap(), "https://github.com/owner/repo.git");
    }

    #[tokio::test]
    async fn resolve_remote_url_returns_ssh_url() {
        let dir = init_repo_with_remote("origin", "git@github.com:owner/repo.git");
        let result = resolve_remote_url(dir.path(), "origin").await;
        assert_eq!(result.unwrap(), "git@github.com:owner/repo.git");
    }

    #[tokio::test]
    async fn resolve_remote_url_errors_when_remote_not_found() {
        let dir = init_repo_with_remote("origin", "https://github.com/owner/repo.git");
        let result = resolve_remote_url(dir.path(), "upstream").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn resolve_remote_url_errors_when_path_is_not_a_repo() {
        let dir = TempDir::new().unwrap();
        let result = resolve_remote_url(dir.path(), "origin").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn resolve_remote_url_errors_when_path_does_not_exist() {
        let result = resolve_remote_url(Path::new("/nonexistent/path"), "origin").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn resolve_remote_url_uses_configured_remote_name() {
        let dir = init_repo_with_remote("upstream", "https://github.com/other/repo.git");
        let result = resolve_remote_url(dir.path(), "upstream").await;
        assert_eq!(result.unwrap(), "https://github.com/other/repo.git");
    }

    #[tokio::test]
    async fn resolve_remote_url_errors_when_repo_has_no_remotes() {
        let dir = TempDir::new().unwrap();
        Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let result = resolve_remote_url(dir.path(), "origin").await;
        assert!(result.is_err());
    }
}
