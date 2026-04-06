use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use tracing::info;

use super::gh_github::apply_git_identity_from_gh;
use super::traits::ExternalActions;
use crate::error::{MaestroError, Result};
use crate::git::worktree_remove;
use crate::jira::acli;
use crate::process::{self, CommandOutput};

pub struct DryRunActions {
    pub repo_path: PathBuf,
    git_remote: String,
    acli_extra_prefixes: Vec<Vec<String>>,
}

impl DryRunActions {
    pub fn new(
        repo_path: PathBuf,
        git_remote: String,
        acli_extra_prefixes: Vec<Vec<String>>,
    ) -> Self {
        Self {
            repo_path,
            git_remote,
            acli_extra_prefixes,
        }
    }
}

#[async_trait]
impl ExternalActions for DryRunActions {
    async fn assign_ticket(&self, key: &str) -> Result<()> {
        info!(
            ticket = key,
            "[DRY] Would assign ticket to current Jira user (acli @me)"
        );
        Ok(())
    }

    async fn transition_ticket(&self, key: &str, status: &str) -> Result<()> {
        info!(
            ticket = key,
            status = status,
            "[DRY] Would transition ticket"
        );
        Ok(())
    }

    async fn unassign_ticket(&self, key: &str) -> Result<()> {
        info!(ticket = key, "[DRY] Would unassign ticket");
        Ok(())
    }

    async fn get_ticket_details(&self, key: &str) -> Result<String> {
        info!(
            ticket = key,
            "Retrieving ticket details (dry mode — read-only, executes normally)"
        );
        let output = acli::run_acli_checked(
            &[
                "jira",
                "workitem",
                "view",
                key,
                "--json",
                "--fields",
                "key,issuetype,summary,status,assignee,description",
            ],
            &self.acli_extra_prefixes,
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
        info!(
            branch = branch,
            base = base,
            path = %worktree_path.display(),
            "Creating git worktree (dry mode — local operation, executes normally)"
        );

        if let Some(parent) = worktree_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        if worktree_path.exists() {
            info!(path = %worktree_path.display(), "Worktree already exists, reusing");
            return Ok(worktree_path);
        }

        let remote = &self.git_remote;
        info!(base = base, remote = %remote, "Fetching base branch from git remote");
        let fetch_output = process::run_shell_command(
            &format!("git fetch {remote} {base}"),
            &self.repo_path,
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
            &self.repo_path,
            CancellationToken::new(),
        )
        .await?;

        if !output.success() {
            let output2 = process::run_shell_command(
                &format!("git worktree add {} {branch}", worktree_path.display()),
                &self.repo_path,
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

    async fn remove_worktree(&self, path: &Path) -> Result<()> {
        info!(
            path = %path.display(),
            "Removing git worktree (dry mode — local operation, executes normally)"
        );
        worktree_remove::remove_git_worktree(&self.repo_path, path).await
    }

    async fn create_pr(
        &self,
        title: &str,
        _body: &str,
        branch: &str,
        _base: &str,
    ) -> Result<String> {
        info!(
            title = title,
            branch = branch,
            remote = %self.git_remote,
            "[DRY] Would push branch to remote and create pull request"
        );
        Ok(format!("https://dry-run/pr/{branch}"))
    }

    async fn commit_changes(&self, cwd: &Path, message: &str) -> Result<()> {
        info!(
            cwd = %cwd.display(),
            message = message,
            "Committing changes (dry mode — local operation, executes normally)"
        );

        let add_output =
            process::run_shell_command("git add -A", cwd, CancellationToken::new()).await?;
        if !add_output.success() {
            return Err(MaestroError::Git(format!(
                "Failed to stage changes: {}",
                add_output.stderr
            )));
        }

        let status_output =
            process::run_shell_command("git diff --cached --quiet", cwd, CancellationToken::new())
                .await?;
        if status_output.success() {
            info!("No changes to commit");
            return Ok(());
        }

        let escaped_message = message.replace('"', r#"\""#);
        let commit_output = process::run_shell_command(
            &format!("git commit -m \"{escaped_message}\""),
            cwd,
            CancellationToken::new(),
        )
        .await?;
        if !commit_output.success() {
            return Err(MaestroError::Git(format!(
                "Failed to commit: {}",
                commit_output.stderr
            )));
        }
        Ok(())
    }

    async fn configure_git_author_from_github(&self, cwd: &Path) -> Result<()> {
        info!(
            cwd = %cwd.display(),
            "Aligning git author with gh (dry mode — local git config, executes normally)"
        );
        apply_git_identity_from_gh(cwd, CancellationToken::new()).await
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
