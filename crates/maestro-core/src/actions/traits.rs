use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::error::Result;
use crate::process::CommandOutput;

#[async_trait]
pub trait ExternalActions: Send + Sync {
    // Jira
    async fn assign_ticket(&self, key: &str, user: &str) -> Result<()>;
    async fn transition_ticket(&self, key: &str, status: &str) -> Result<()>;
    async fn unassign_ticket(&self, key: &str) -> Result<()>;
    async fn get_ticket_details(&self, key: &str) -> Result<String>;

    // Git/GitHub
    async fn create_worktree(&self, branch: &str, base: &str) -> Result<PathBuf>;
    async fn remove_worktree(&self, path: &Path) -> Result<()>;
    async fn create_pr(&self, title: &str, body: &str, branch: &str, base: &str) -> Result<String>;
    async fn commit_changes(&self, cwd: &Path, message: &str) -> Result<()>;

    // Shell commands (lint, test)
    async fn run_command(&self, cmd: &str, cwd: &Path) -> Result<CommandOutput>;
}
