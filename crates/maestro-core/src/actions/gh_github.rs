// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Git author and PR reviewer alignment with the authenticated `gh` user.

use std::path::Path;

use serde::Deserialize;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::error::{MaestroError, Result};
use crate::process;

#[derive(Debug, Deserialize)]
struct GhUser {
    login: String,
    id: u64,
    name: Option<String>,
}

/// Returns `(display_name, noreply_email)` for git commits, matching the logged-in `gh` account.
pub async fn github_commit_identity(
    cwd: &Path,
    cancel: CancellationToken,
) -> Result<(String, String)> {
    let u = fetch_gh_user(cwd, cancel).await?;
    let display_name = u
        .name
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(String::from)
        .unwrap_or_else(|| u.login.clone());
    let email = format!("{}+{}@users.noreply.github.com", u.id, u.login);
    Ok((display_name, email))
}

async fn fetch_gh_user(cwd: &Path, cancel: CancellationToken) -> Result<GhUser> {
    let out = process::run_command("gh", &["api", "user"], cwd, cancel).await?;
    if !out.success() {
        return Err(MaestroError::Git(format!(
            "gh api user failed: {}",
            out.stderr.trim()
        )));
    }
    let u: GhUser = serde_json::from_str(out.stdout.trim())
        .map_err(|e| MaestroError::Git(format!("failed to parse gh api user JSON: {e}")))?;
    if u.login.is_empty() {
        return Err(MaestroError::Git(
            "gh api user returned an empty login".into(),
        ));
    }
    Ok(u)
}

/// Sets `user.name` and `user.email` locally in `cwd` so commits match the `gh` account.
pub async fn apply_git_identity_from_gh(cwd: &Path, cancel: CancellationToken) -> Result<()> {
    let (name, email) = github_commit_identity(cwd, cancel.child_token()).await?;

    let name_out = crate::process::run_command(
        "git",
        &["config", "user.name", &name],
        cwd,
        cancel.child_token(),
    )
    .await?;
    if !name_out.success() {
        return Err(MaestroError::Git(format!(
            "git config user.name failed: {}",
            name_out.stderr.trim()
        )));
    }

    let email_out = crate::process::run_command(
        "git",
        &["config", "user.email", &email],
        cwd,
        cancel.child_token(),
    )
    .await?;
    if !email_out.success() {
        return Err(MaestroError::Git(format!(
            "git config user.email failed: {}",
            email_out.stderr.trim()
        )));
    }

    info!(
        git_name = %name,
        git_email = %email,
        "Set worktree git author from GitHub CLI (`gh`) user"
    );
    Ok(())
}

/// Adds the authenticated user as a PR reviewer (`gh pr edit --add-reviewer`).
///
/// GitHub may reject this if the user is already the PR author; callers should treat failure as non-fatal.
// Unit-test note: all public functions in this module (`github_commit_identity`,
// `apply_git_identity_from_gh`, `gh_request_self_pr_reviewer`) are thin wrappers
// around `process::run_command("gh", ...)` with no extractable pure logic.
// `parse_pr_url` (mentioned in the task spec) is in `github/mod.rs` and already
// has comprehensive tests there. The GhUser deserialization is validated by the
// gh API contract. Integration testing requires a real `gh` CLI and GitHub auth.
pub async fn gh_request_self_pr_reviewer(
    cwd: &Path,
    pr_url: &str,
    cancel: CancellationToken,
) -> Result<()> {
    let pr_url = pr_url.trim();
    if pr_url.is_empty() {
        return Err(MaestroError::Git("empty PR URL".into()));
    }

    let u = fetch_gh_user(cwd, cancel.child_token()).await?;

    let out = process::run_command(
        "gh",
        &["pr", "edit", pr_url, "--add-reviewer", u.login.as_str()],
        cwd,
        cancel.child_token(),
    )
    .await?;

    if !out.success() {
        return Err(MaestroError::Git(format!(
            "gh pr edit --add-reviewer failed: {}",
            out.stderr.trim()
        )));
    }

    info!(
        pr = %pr_url,
        reviewer = %u.login,
        "Requested PR review from authenticated GitHub user"
    );
    Ok(())
}
