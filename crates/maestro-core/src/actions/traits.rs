// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

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
    /// Best-effort **`git branch -D`** in the main repo after worktree removal (no-op if `branch` is empty or already gone).
    async fn delete_local_branch(&self, branch: &str) -> Result<()>;
    async fn create_pr(&self, title: &str, body: &str, branch: &str, base: &str) -> Result<String>;
    async fn commit_changes(&self, cwd: &Path, message: &str) -> Result<()>;

    /// Set `git config user.name` / `user.email` in `cwd` from `gh api user` (GitHub no-reply email).
    async fn configure_git_author_from_github(&self, cwd: &Path) -> Result<()>;

    /// Return a fresh GitHub App installation token for injection as `GH_TOKEN` into worker
    /// container environments. Returns `None` when the GitHub App is not configured or when
    /// a token fetch fails (caller falls back to the personal `gh` user in that case).
    async fn get_gh_installation_token(&self, cwd: &Path) -> Option<String>;

    /// Request the authenticated `gh` user as a reviewer on `pr_url` (`gh pr edit --add-reviewer`).
    /// Returns `Ok(true)` if `gh` reported success, `Ok(false)` if skipped (e.g. dry mode), `Err` if `gh` failed.
    async fn request_github_self_as_pr_reviewer(&self, cwd: &Path, pr_url: &str) -> Result<bool>;

    // Shell commands (e.g. invoked by other actions or tests)
    async fn run_command(&self, cmd: &str, cwd: &Path) -> Result<CommandOutput>;
}
