// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use tracing::info;

use super::gh_github::{apply_git_identity_from_gh, gh_request_self_pr_reviewer};
use super::traits::ExternalActions;
use crate::error::{MaestroError, Result};
use crate::git::worktree_remove;

use crate::github_app::GitHubAppTokenManager;
use crate::process::{self, CommandOutput};

pub struct RealActions {
    pub repo_path: PathBuf,
    git_remote: String,
    github_app: Option<Arc<GitHubAppTokenManager>>,
}

impl RealActions {
    pub fn new(
        repo_path: PathBuf,
        git_remote: String,
        github_app: Option<Arc<GitHubAppTokenManager>>,
    ) -> Self {
        Self {
            repo_path,
            git_remote,
            github_app,
        }
    }

    /// Return `[("GH_TOKEN", token)]` when a GitHub App is configured, otherwise empty.
    /// Used to inject credentials into git/gh subprocesses spawned in the main process.
    async fn gh_token_env(&self) -> Vec<(String, String)> {
        let Some(app) = &self.github_app else {
            return vec![];
        };
        match app.get_installation_token(&self.repo_path).await {
            Ok(token) => vec![("GH_TOKEN".to_string(), token)],
            Err(e) => {
                tracing::warn!(error = %e, "Failed to get GitHub App token for env injection");
                vec![]
            }
        }
    }
}

#[async_trait]
impl ExternalActions for RealActions {
    async fn assign_ticket(&self, key: &str) -> Result<()> {
        info!(
            ticket = key,
            "Assigning ticket to current Jira user (acli @me)"
        );
        let output = process::run_command(
            "acli",
            &[
                "jira",
                "workitem",
                "assign",
                "--key",
                key,
                "--assignee",
                "@me",
                "--yes",
            ],
            &self.repo_path,
            CancellationToken::new(),
        )
        .await?;
        if !output.success() {
            return Err(MaestroError::Jira(format!(
                "Failed to assign ticket {key}: {}",
                output.stderr
            )));
        }
        Ok(())
    }

    async fn transition_ticket(&self, key: &str, status: &str) -> Result<()> {
        info!(ticket = key, status = status, "Transitioning ticket");
        let output = process::run_command(
            "acli",
            &[
                "jira",
                "workitem",
                "transition",
                "--key",
                key,
                "--status",
                status,
                "--yes",
            ],
            &self.repo_path,
            CancellationToken::new(),
        )
        .await?;
        if !output.success() {
            return Err(MaestroError::Jira(format!(
                "Failed to transition ticket {key} to {status}: {}",
                output.stderr
            )));
        }
        Ok(())
    }

    async fn unassign_ticket(&self, key: &str) -> Result<()> {
        info!(ticket = key, "Unassigning ticket");
        let output = process::run_command(
            "acli",
            &[
                "jira",
                "workitem",
                "assign",
                "--key",
                key,
                "--remove-assignee",
                "--yes",
            ],
            &self.repo_path,
            CancellationToken::new(),
        )
        .await?;
        if !output.success() {
            return Err(MaestroError::Jira(format!(
                "Failed to unassign ticket {key}: {}",
                output.stderr
            )));
        }
        Ok(())
    }

    async fn get_ticket_details(&self, key: &str) -> Result<String> {
        info!(ticket = key, "Retrieving ticket details");
        let output = process::run_command(
            "acli",
            &[
                "jira",
                "workitem",
                "view",
                key,
                "--json",
                "--fields",
                "key,issuetype,summary,status,assignee,description",
            ],
            &self.repo_path,
            CancellationToken::new(),
        )
        .await?;
        if !output.success() {
            return Err(MaestroError::Jira(format!(
                "Failed to get ticket details for {key}: {}",
                output.stderr
            )));
        }
        Ok(output.stdout)
    }

    async fn create_worktree(&self, branch: &str, base: &str) -> Result<PathBuf> {
        let worktree_path = self
            .repo_path
            .join("worktrees")
            .join(branch.replace('/', "-"));
        info!(branch = branch, base = base, path = %worktree_path.display(), "Creating git worktree");

        // Create worktrees directory if it doesn't exist
        if let Some(parent) = worktree_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        worktree_remove::clear_worktree_path_for_recreate(&self.repo_path, &worktree_path).await?;

        let remote = &self.git_remote;
        // Fetch the base branch from the configured remote to ensure it's available locally.
        // Inject GH_TOKEN so git's credential helper (gh) can authenticate via the GitHub App.
        info!(base = base, remote = %remote, "Fetching base branch from git remote");
        let token_env = self.gh_token_env().await;
        let token_env_refs: Vec<(&str, &str)> =
            token_env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let fetch_output = process::run_shell_command_with_env(
            &format!("git fetch {remote} {base}"),
            &self.repo_path,
            CancellationToken::new(),
            &token_env_refs,
        )
        .await?;
        if !fetch_output.success() {
            return Err(MaestroError::Git(format!(
                "Failed to fetch base branch '{}': {}",
                base, fetch_output.stderr
            )));
        }

        // Create worktree from <remote>/<base>.
        // IMPORTANT: use run_shell_command_with_env (plain sh, no mise exec wrapper) so that
        // mise does NOT try to install tools before the git command runs — the main container
        // lacks write access to /usr/local/rustup/tmp/ and would fail.  Tools are installed
        // later inside an isolated worker container that has the correct volume mounts.
        // Also suppress git hooks (-c core.hooksPath=/dev/null) for the same reason.
        let output = process::run_shell_command_with_env(
            &format!(
                "git -c core.hooksPath=/dev/null worktree add -b {branch} {} {remote}/{base}",
                worktree_path.display()
            ),
            &self.repo_path,
            CancellationToken::new(),
            &[],
        )
        .await?;

        if !output.success() {
            // Branch might already exist, try without -b
            let output2 = process::run_shell_command_with_env(
                &format!(
                    "git -c core.hooksPath=/dev/null worktree add {} {branch}",
                    worktree_path.display()
                ),
                &self.repo_path,
                CancellationToken::new(),
                &[],
            )
            .await?;
            if !output2.success() {
                return Err(MaestroError::Git(format!(
                    "Failed to create worktree: {}",
                    output2.stderr
                )));
            }
        }

        Ok(worktree_path)
    }

    async fn remove_worktree(&self, path: &Path) -> Result<()> {
        worktree_remove::remove_git_worktree(&self.repo_path, path).await
    }

    async fn delete_local_branch(&self, branch: &str) -> Result<()> {
        let branch = branch.trim();
        if branch.is_empty() {
            return Ok(());
        }
        info!(branch = %branch, "Deleting local git branch");
        let output = process::run_command(
            "git",
            &["branch", "-D", branch],
            &self.repo_path,
            CancellationToken::new(),
        )
        .await?;
        if output.success() {
            return Ok(());
        }
        let stderr = output.stderr.to_lowercase();
        if stderr.contains("not found")
            || stderr.contains("no branch named")
            || stderr.contains("invalid branch name")
        {
            return Ok(());
        }
        Err(MaestroError::Git(format!(
            "Failed to delete branch {branch}: {}",
            output.stderr
        )))
    }

    async fn configure_git_author_from_github(&self, cwd: &Path) -> Result<()> {
        if let Some(ref app) = self.github_app {
            return app
                .configure_git_and_gh_auth(cwd, CancellationToken::new())
                .await;
        }
        apply_git_identity_from_gh(cwd, CancellationToken::new()).await
    }

    async fn get_gh_installation_token(&self, cwd: &Path) -> Option<String> {
        let app = self.github_app.as_ref()?;
        match app.get_token_for_injection(cwd).await {
            Ok(token) => Some(token),
            Err(e) => {
                tracing::warn!(error = %e, "Failed to fetch GitHub App installation token for worker injection");
                None
            }
        }
    }

    async fn request_github_self_as_pr_reviewer(&self, cwd: &Path, pr_url: &str) -> Result<bool> {
        // GitHub App bot accounts cannot be added as reviewers — skip silently.
        if self.github_app.is_some() {
            return Ok(false);
        }
        gh_request_self_pr_reviewer(cwd, pr_url, CancellationToken::new()).await?;
        Ok(true)
    }

    async fn run_command(&self, cmd: &str, cwd: &Path) -> Result<CommandOutput> {
        info!(cmd = cmd, cwd = %cwd.display(), "Running command");
        process::run_shell_command(cmd, cwd, CancellationToken::new()).await
    }
}
