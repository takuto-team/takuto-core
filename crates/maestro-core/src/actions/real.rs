use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use tracing::info;

use super::traits::ExternalActions;
use crate::error::{MaestroError, Result};
use crate::process::{self, CommandOutput};

pub struct RealActions {
    pub repo_path: PathBuf,
}

impl RealActions {
    pub fn new(repo_path: PathBuf) -> Self {
        Self { repo_path }
    }
}

#[async_trait]
impl ExternalActions for RealActions {
    async fn assign_ticket(&self, key: &str, user: &str) -> Result<()> {
        info!(ticket = key, user = user, "Assigning ticket");
        let output = process::run_shell_command(
            &format!("acli jira issue assign {key} {user}"),
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
        let output = process::run_shell_command(
            &format!("acli jira issue transition {key} \"{status}\""),
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
        let output = process::run_shell_command(
            &format!("acli jira issue assign {key} --unassign"),
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
        let output = process::run_shell_command(
            &format!("acli jira issue view {key} --output json"),
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
        let worktree_path = self.repo_path.join("worktrees").join(branch.replace('/', "-"));
        info!(branch = branch, base = base, path = %worktree_path.display(), "Creating git worktree");

        // Create worktrees directory if it doesn't exist
        if let Some(parent) = worktree_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        // Check if worktree already exists
        if worktree_path.exists() {
            info!(path = %worktree_path.display(), "Worktree already exists, reusing");
            return Ok(worktree_path);
        }

        let output = process::run_shell_command(
            &format!(
                "git worktree add -b {branch} {} {base}",
                worktree_path.display()
            ),
            &self.repo_path,
            CancellationToken::new(),
        )
        .await?;

        if !output.success() {
            // Branch might already exist, try without -b
            let output2 = process::run_shell_command(
                &format!(
                    "git worktree add {} {branch}",
                    worktree_path.display()
                ),
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
        info!(path = %path.display(), "Removing git worktree");
        let output = process::run_shell_command(
            &format!("git worktree remove {}", path.display()),
            &self.repo_path,
            CancellationToken::new(),
        )
        .await?;
        if !output.success() {
            return Err(MaestroError::Git(format!(
                "Failed to remove worktree: {}",
                output.stderr
            )));
        }
        Ok(())
    }

    async fn create_pr(
        &self,
        title: &str,
        body: &str,
        branch: &str,
        base: &str,
    ) -> Result<String> {
        info!(title = title, branch = branch, base = base, "Creating pull request");

        // Push branch first
        let push_output = process::run_shell_command(
            &format!("git push -u origin {branch}"),
            &self.repo_path,
            CancellationToken::new(),
        )
        .await?;
        if !push_output.success() {
            return Err(MaestroError::Git(format!(
                "Failed to push branch {branch}: {}",
                push_output.stderr
            )));
        }

        // Create PR via gh
        let escaped_title = title.replace('"', r#"\""#);
        let escaped_body = body.replace('"', r#"\""#);
        let output = process::run_shell_command(
            &format!(
                "gh pr create --title \"{escaped_title}\" --body \"{escaped_body}\" --base {base} --head {branch}"
            ),
            &self.repo_path,
            CancellationToken::new(),
        )
        .await?;
        if !output.success() {
            return Err(MaestroError::Git(format!(
                "Failed to create PR: {}",
                output.stderr
            )));
        }
        // gh pr create outputs the PR URL on stdout
        Ok(output.stdout.trim().to_string())
    }

    async fn commit_changes(&self, cwd: &Path, message: &str) -> Result<()> {
        info!(cwd = %cwd.display(), message = message, "Committing changes");

        // Stage all changes
        let add_output = process::run_shell_command(
            "git add -A",
            cwd,
            CancellationToken::new(),
        )
        .await?;
        if !add_output.success() {
            return Err(MaestroError::Git(format!(
                "Failed to stage changes: {}",
                add_output.stderr
            )));
        }

        // Check if there's anything to commit
        let status_output = process::run_shell_command(
            "git diff --cached --quiet",
            cwd,
            CancellationToken::new(),
        )
        .await?;
        if status_output.success() {
            info!("No changes to commit");
            return Ok(());
        }

        // Commit
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

    async fn run_command(&self, cmd: &str, cwd: &Path) -> Result<CommandOutput> {
        info!(cmd = cmd, cwd = %cwd.display(), "Running command");
        process::run_shell_command(cmd, cwd, CancellationToken::new()).await
    }
}
