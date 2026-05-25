// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use tracing::info;

use super::gh_github::apply_git_identity_from_gh;
use super::traits::ExternalActions;
use crate::error::{MaestroError, Result};
use crate::git::worktree_remove;
use crate::github_app::GitHubAppTokenManager;
use crate::jira::JiraError;
use crate::process::{self, CommandOutput};

pub struct DryRunActions {
    git_remote: String,
    github_app: Option<Arc<GitHubAppTokenManager>>,
}

impl DryRunActions {
    pub fn new(git_remote: String, github_app: Option<Arc<GitHubAppTokenManager>>) -> Self {
        Self {
            git_remote,
            github_app,
        }
    }
}

#[async_trait]
impl ExternalActions for DryRunActions {
    async fn assign_ticket(&self, _repo_path: &Path, key: &str) -> Result<()> {
        info!(
            ticket = key,
            "[DRY] Would assign ticket to current Jira user (acli @me)"
        );
        Ok(())
    }

    async fn transition_ticket(&self, _repo_path: &Path, key: &str, status: &str) -> Result<()> {
        info!(
            ticket = key,
            status = status,
            "[DRY] Would transition ticket"
        );
        Ok(())
    }

    async fn unassign_ticket(&self, _repo_path: &Path, key: &str) -> Result<()> {
        info!(ticket = key, "[DRY] Would unassign ticket");
        Ok(())
    }

    async fn get_ticket_details(&self, repo_path: &Path, key: &str) -> Result<String> {
        info!(
            ticket = key,
            "Retrieving ticket details (dry mode — read-only, executes normally)"
        );
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
            repo_path,
            CancellationToken::new(),
        )
        .await?;
        if !output.success() {
            return Err(JiraError::GetDetailsFailed {
                key: key.to_string(),
                stderr: output.stderr,
            }
            .into());
        }
        Ok(output.stdout)
    }

    async fn create_worktree(
        &self,
        repo_path: &Path,
        branch: &str,
        base: &str,
    ) -> Result<PathBuf> {
        let worktree_path = repo_path.join("worktrees").join(branch.replace('/', "-"));
        info!(
            branch = branch,
            base = base,
            path = %worktree_path.display(),
            "Creating git worktree (dry mode — local operation, executes normally)"
        );

        if let Some(parent) = worktree_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        worktree_remove::clear_worktree_path_for_recreate(repo_path, &worktree_path).await?;

        let remote = &self.git_remote;
        info!(base = base, remote = %remote, "Fetching base branch from git remote");
        let fetch_output = process::run_shell_command(
            &format!("git fetch {remote} {base}"),
            repo_path,
            CancellationToken::new(),
        )
        .await?;
        if !fetch_output.success() {
            return Err(MaestroError::Git(format!(
                "Failed to fetch base branch '{}': {}",
                base, fetch_output.stderr
            )));
        }

        let output = process::run_shell_command(
            &format!(
                "git worktree add -b {branch} {} {remote}/{base}",
                worktree_path.display()
            ),
            repo_path,
            CancellationToken::new(),
        )
        .await?;

        if !output.success() {
            let output2 = process::run_shell_command(
                &format!("git worktree add {} {branch}", worktree_path.display()),
                repo_path,
                CancellationToken::new(),
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

    async fn remove_worktree(&self, repo_path: &Path, worktree_path: &Path) -> Result<()> {
        info!(
            path = %worktree_path.display(),
            "Removing git worktree (dry mode — local operation, executes normally)"
        );
        worktree_remove::remove_git_worktree(repo_path, worktree_path).await
    }

    async fn delete_local_branch(&self, repo_path: &Path, branch: &str) -> Result<()> {
        let branch = branch.trim();
        if branch.is_empty() {
            return Ok(());
        }
        info!(
            branch = %branch,
            "Deleting local git branch (dry mode — local operation, executes normally)"
        );
        let output = process::run_command(
            "git",
            &["branch", "-D", branch],
            repo_path,
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
            info!(
                cwd = %cwd.display(),
                "Configuring GitHub App bot identity (dry mode — local git config, executes normally)"
            );
            return app
                .configure_git_and_gh_auth(cwd, CancellationToken::new())
                .await;
        }
        info!(
            cwd = %cwd.display(),
            "Aligning git author with gh (dry mode — local git config, executes normally)"
        );
        apply_git_identity_from_gh(cwd, CancellationToken::new()).await
    }

    async fn get_gh_installation_token(&self, cwd: &Path) -> Option<String> {
        let app = self.github_app.as_ref()?;
        match app.get_token_for_injection(cwd).await {
            Ok(token) => Some(token),
            Err(e) => {
                info!(error = %e, "[DRY] GitHub App token fetch failed");
                None
            }
        }
    }

    async fn request_github_self_as_pr_reviewer(&self, cwd: &Path, pr_url: &str) -> Result<bool> {
        info!(
            cwd = %cwd.display(),
            pr = %pr_url,
            "[DRY] Would request authenticated GitHub user as PR reviewer"
        );
        Ok(false)
    }

    async fn run_command(&self, cmd: &str, cwd: &Path) -> Result<CommandOutput> {
        info!(
            cmd = cmd,
            cwd = %cwd.display(),
            "Running command (dry mode — local operation, executes normally)"
        );
        process::run_shell_command(cmd, cwd, CancellationToken::new()).await
    }
}
