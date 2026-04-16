//! Remove a registered git worktree with `git worktree remove --force`.
//!
//! Isolated Docker workers historically ran some commands as root (`docker run` default user),
//! leaving root-owned files on bind-mounted worktrees. The main Maestro process then could not
//! delete those paths. We retry after a non-interactive `sudo bash -c chown` when Git reports
//! permission denied (Maestro image sudoers allow this).

use std::path::Path;

use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::error::{MaestroError, Result};
use crate::process;

/// Compute the Claude Code session folder for a given repo path.
/// Claude Code encodes absolute paths by replacing `/` with `-`.
/// E.g., `/Users/user/dev/maestro` → `.claude/projects/-Users-user-dev-maestro`
fn compute_session_folder(repo_path: &Path) -> std::path::PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("~"));
    let projects_dir = home.join(".claude/projects");

    // Convert to absolute path string and encode by replacing / with -
    let abs_path = repo_path.display().to_string();
    let encoded = abs_path.replace('/', "-");

    projects_dir.join(&encoded)
}

/// Single-quote `s` for safe embedding in `bash -c` (paths may contain spaces or `'`).
fn bash_single_quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

/// Delete Claude Code session folder to force a fresh session on next workflow run.
/// Sessions are stored by absolute path, so deleting the folder prevents resumption
/// of old sessions when a branch is recreated.
async fn cleanup_claude_code_session(repo_path: &Path) {
    let session_folder = compute_session_folder(repo_path);
    match tokio::fs::remove_dir_all(&session_folder).await {
        Ok(()) => {
            info!(
                path = %session_folder.display(),
                "Deleted Claude Code session folder (next workflow run will start fresh)"
            );
        }
        Err(e) => {
            // Session folder may not exist or may be in use; not a fatal error
            warn!(
                error = %e,
                path = %session_folder.display(),
                "Could not delete Claude Code session folder (may be in use by another process)"
            );
        }
    }
}

pub async fn remove_git_worktree(repo_path: &Path, worktree_path: &Path) -> Result<()> {
    let path_str = worktree_path.to_str().ok_or_else(|| {
        MaestroError::Git(format!(
            "worktree path is not valid UTF-8: {}",
            worktree_path.display()
        ))
    })?;

    info!(path = %worktree_path.display(), "Removing git worktree");

    let output = git_worktree_remove(repo_path, path_str).await?;
    if output.success() {
        // Worktree removed successfully; clean up Claude Code session folder
        // so next run starts fresh instead of resuming old session
        cleanup_claude_code_session(repo_path).await;
        return Ok(());
    }

    let stderr_lower = output.stderr.to_lowercase();
    #[cfg(unix)]
    if stderr_lower.contains("permission denied") {
        if chown_tree_to_effective_user_via_sudo(repo_path, path_str).await {
            let output2 = git_worktree_remove(repo_path, path_str).await?;
            if output2.success() {
                info!(
                    path = path_str,
                    "Removed worktree after chown fallback (fixing root-owned files from isolated runs)"
                );
                // Clean up Claude Code session folder after successful fallback removal
                cleanup_claude_code_session(repo_path).await;
                return Ok(());
            }
            return Err(MaestroError::Git(format!(
                "Failed to remove worktree: {}",
                output2.stderr
            )));
        }
        warn!(
            path = path_str,
            "git worktree remove failed: permission denied; sudo chown fallback skipped or failed"
        );
    }

    Err(MaestroError::Git(format!(
        "Failed to remove worktree: {}",
        output.stderr
    )))
}

/// Remove a leftover worktree directory before `git worktree add` for a **new** workflow.
///
/// If the dashboard row was removed but disk cleanup failed (or the tree was never registered),
/// reusing the path would keep another ticket’s files while Jira prompts match the new key.
pub async fn clear_worktree_path_for_recreate(
    repo_path: &Path,
    worktree_path: &Path,
) -> Result<()> {
    if !worktree_path.exists() {
        return Ok(());
    }
    info!(
        path = %worktree_path.display(),
        "Existing worktree path on disk — clearing so this workflow gets a fresh tree from base"
    );
    match remove_git_worktree(repo_path, worktree_path).await {
        Ok(()) => Ok(()),
        Err(git_err) => {
            warn!(
                error = %git_err,
                path = %worktree_path.display(),
                "git worktree remove failed (unregistered or corrupt tree); removing directory from disk"
            );
            tokio::fs::remove_dir_all(worktree_path).await.map_err(|io_err| {
                MaestroError::Git(format!(
                    "Could not remove stale worktree directory {}: {} (after git error: {git_err})",
                    worktree_path.display(),
                    io_err
                ))
            })
        }
    }
}

async fn git_worktree_remove(
    repo_path: &Path,
    path_str: &str,
) -> Result<crate::process::CommandOutput> {
    process::run_command(
        "git",
        &["worktree", "remove", "--force", path_str],
        repo_path,
        CancellationToken::new(),
    )
    .await
}

#[cfg(unix)]
async fn chown_tree_to_effective_user_via_sudo(repo_path: &Path, path_str: &str) -> bool {
    let uid = unsafe { libc::getuid() };
    let gid = unsafe { libc::getgid() };
    if uid == 0 {
        return false;
    }

    let script = format!(
        "chown -R {}:{} -- {}",
        uid,
        gid,
        bash_single_quoted(path_str)
    );

    let output = match process::run_command(
        "sudo",
        &["-n", "/bin/bash", "-c", &script],
        repo_path,
        CancellationToken::new(),
    )
    .await
    {
        Ok(o) => o,
        Err(e) => {
            warn!(error = %e, "could not spawn sudo for worktree chown fallback");
            return false;
        }
    };

    if !output.success() {
        tracing::debug!(
            stderr = %output.stderr,
            "sudo chown fallback for worktree teardown did not succeed"
        );
    }
    output.success()
}

#[cfg(test)]
mod tests {
    use super::{bash_single_quoted, compute_session_folder};
    use std::path::Path;

    #[test]
    fn bash_single_quote_escapes_apostrophe() {
        assert_eq!(bash_single_quoted("a"), "'a'");
        assert_eq!(bash_single_quoted("it's"), "'it'\\''s'");
    }

    #[test]
    fn compute_session_folder_encodes_path_correctly() {
        let repo_path = Path::new("/Users/alexanderobellianne/dev/maestro");
        let session_folder = compute_session_folder(repo_path);

        // Should contain the encoded path
        let path_str = session_folder.to_str().unwrap();
        assert!(path_str.contains(".claude/projects"));
        assert!(path_str.contains("-Users-alexanderobellianne-dev-maestro"));
    }
}
