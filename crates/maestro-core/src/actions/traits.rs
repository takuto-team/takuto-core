// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::error::Result;
use crate::process::CommandOutput;

#[async_trait]
pub trait ExternalActions: Send + Sync {
    // Jira
    //
    // Plan-10: ticket-scoped Jira methods now take `repo_path: &Path` explicitly
    // so callers thread the workflow's repository row rather than reading a
    // global `cfg.git.repo_path`. The path is the cwd for the `acli` subprocess
    // (acli reads the workspace context from the cwd it's spawned in).
    async fn assign_ticket(&self, repo_path: &Path, key: &str) -> Result<()>;
    async fn transition_ticket(&self, repo_path: &Path, key: &str, status: &str) -> Result<()>;
    async fn unassign_ticket(&self, repo_path: &Path, key: &str) -> Result<()>;
    async fn get_ticket_details(&self, repo_path: &Path, key: &str) -> Result<String>;

    // Git/GitHub
    //
    // Plan-10: worktree-scoped operations now take `repo_path: &Path` explicitly.
    // The implementor no longer reads `self.repo_path()` (a global config getter
    // that was dropped); callers resolve the repository's `local_path` from the
    // workflow's `repository_id` and thread it in.
    async fn create_worktree(
        &self,
        repo_path: &Path,
        branch: &str,
        base: &str,
    ) -> Result<PathBuf>;
    async fn remove_worktree(&self, repo_path: &Path, worktree_path: &Path) -> Result<()>;
    /// Best-effort **`git branch -D`** in the main repo after worktree removal (no-op if `branch` is empty or already gone).
    async fn delete_local_branch(&self, repo_path: &Path, branch: &str) -> Result<()>;
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
