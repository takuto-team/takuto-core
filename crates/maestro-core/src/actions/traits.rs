use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::error::Result;
use crate::process::CommandOutput;

#[async_trait]
pub trait ExternalActions: Send + Sync {
    // Jira
    /// Assign the work item to the **currently authenticated Jira user** (acli `@me`).
    async fn assign_ticket(&self, key: &str) -> Result<()>;
    async fn transition_ticket(&self, key: &str, status: &str) -> Result<()>;
    async fn unassign_ticket(&self, key: &str) -> Result<()>;
    async fn get_ticket_details(&self, key: &str) -> Result<String>;

    // Git/GitHub
    async fn create_worktree(&self, branch: &str, base: &str) -> Result<PathBuf>;
    async fn remove_worktree(&self, path: &Path) -> Result<()>;
    async fn create_pr(&self, title: &str, body: &str, branch: &str, base: &str) -> Result<String>;
    async fn commit_changes(&self, cwd: &Path, message: &str) -> Result<()>;

    /// Set `git config user.name` / `user.email` in `cwd` from `gh api user` (GitHub no-reply email).
    async fn configure_git_author_from_github(&self, cwd: &Path) -> Result<()>;

    /// Request the authenticated `gh` user as a reviewer on `pr_url` (`gh pr edit --add-reviewer`).
    /// Returns `Ok(true)` if `gh` reported success, `Ok(false)` if skipped (e.g. dry mode), `Err` if `gh` failed.
    async fn request_github_self_as_pr_reviewer(&self, cwd: &Path, pr_url: &str) -> Result<bool>;

    // Shell commands (e.g. invoked by other actions or tests)
    async fn run_command(&self, cmd: &str, cwd: &Path) -> Result<CommandOutput>;
}
