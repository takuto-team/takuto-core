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
    use super::bash_single_quoted;

    #[test]
    fn bash_single_quote_escapes_apostrophe() {
        assert_eq!(bash_single_quoted("a"), "'a'");
        assert_eq!(bash_single_quoted("it's"), "'it'\\''s'");
    }
}
