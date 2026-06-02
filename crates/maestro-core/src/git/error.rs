// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Typed errors for git and `gh`-CLI invocations: worktree create/remove,
//! base-branch fetch, branch delete, `gh api user`, `git config user.{name,email}`,
//! PR-reviewer assignment, and the workflow-engine bootstrap steps (`mise install`
//! and worktree-init commands).
//!
//! Replaces the historical `MaestroError::Git(String)` (the `*Str(String)`
//! deprecated shim was removed in the post-§8 #2 cleanup PR).
//! Each variant captures structured operation context —
//! command stderr, branch name, exit code, file path — instead of `format!`-ed
//! sentences. Two sites in `workflow/engine/bootstrap.rs` collapse to direct
//! `?` propagation because they previously wrapped an inner `MaestroError` in
//! a zero-info `"<step> error: {e}"` prefix.
//!
//! See `lore/audits/2026-05-21-clean-code.md` §8 #2 and
//! `lore/audits/2026-05-24-typed-errors-spec.md` for the architecture rules
//! this module follows.

use std::path::PathBuf;

use thiserror::Error;

/// Failures originating from git CLI invocations, `gh` CLI invocations, and
/// the workflow-engine bootstrap steps that orchestrate them.
#[derive(Debug, Error)]
pub enum GitError {
    // ── Worktree create / fetch / delete ──────────────────────────────────
    /// `actions/{real,dry_run}.rs` — `git fetch <base>` exit ≠ 0 while
    /// preparing the worktree's base branch.
    #[error("Failed to fetch base branch '{base}': {stderr}")]
    FetchBaseBranchFailed { base: String, stderr: String },

    /// `actions/{real,dry_run}.rs` — `git worktree add` exit ≠ 0.
    #[error("Failed to create worktree: {stderr}")]
    WorktreeCreateFailed { stderr: String },

    /// `actions/{real,dry_run}.rs` — `git branch -D <branch>` exit ≠ 0
    /// during Mark-as-Done / Delete teardown.
    #[error("Failed to delete branch {branch}: {stderr}")]
    DeleteBranchFailed { branch: String, stderr: String },

    // ── Worktree remove ───────────────────────────────────────────────────
    /// `git/worktree_remove.rs:79` — the worktree path is not valid UTF-8 so
    /// it can't be passed as `&str` to `git worktree remove <path>`.
    #[error("worktree path is not valid UTF-8: {path}")]
    WorktreePathInvalidUtf8 { path: PathBuf },

    /// `git/worktree_remove.rs:109,120` — `git worktree remove --force` exit
    /// ≠ 0 (both first-attempt and retry-after-chown paths share this).
    #[error("Failed to remove worktree: {stderr}")]
    WorktreeRemoveFailed { stderr: String },

    /// `git/worktree_remove.rs:150` — `fs::remove_dir_all(<worktree>)`
    /// fallback also failed after `git worktree remove` reported the tree
    /// as unregistered or corrupt.
    #[error("Could not remove stale worktree directory {path}: {io} (after git error: {git_err})")]
    WorktreeDirRemoveFailed {
        path: PathBuf,
        #[source]
        io: std::io::Error,
        git_err: String,
    },

    // ── `gh` CLI ──────────────────────────────────────────────────────────
    /// `actions/gh_github.rs:48` — `gh api user` exit ≠ 0.
    #[error("gh api user failed: {stderr}")]
    GhApiUserFailed { stderr: String },

    /// `actions/gh_github.rs:54` — `serde_json::from_str` on the `gh api user`
    /// stdout failed. Single-site, so `#[from] serde_json::Error` is legal.
    #[error("failed to parse gh api user JSON: {source}")]
    GhApiUserParseJson {
        #[from]
        source: serde_json::Error,
    },

    /// `actions/gh_github.rs:56` — `gh api user` returned 200 but the login
    /// field is empty.
    #[error("gh api user returned an empty login")]
    GhApiUserEmptyLogin,

    /// `actions/gh_github.rs:119` — caller passed an empty PR URL to
    /// `request_github_self_as_pr_reviewer`.
    #[error("empty PR URL")]
    EmptyPrUrl,

    /// `actions/gh_github.rs:133` — `gh pr edit --add-reviewer <self>`
    /// exit ≠ 0.
    #[error("gh pr edit --add-reviewer failed: {stderr}")]
    GhPrAddReviewerFailed { stderr: String },

    // ── `git config` ──────────────────────────────────────────────────────
    /// `actions/gh_github.rs:75,89` — `git config user.<setting>` exit ≠ 0.
    /// `setting` is a pinned `&'static str` ("name" or "email").
    #[error("git config user.{setting} failed: {stderr}")]
    GitConfigFailed {
        setting: &'static str,
        stderr: String,
    },

    // ── Workflow engine bootstrap steps ───────────────────────────────────
    /// `workflow/engine/bootstrap.rs:547` — `mise install` exit ≠ 0.
    #[error("mise install failed (exit code {exit_code}):\n{stderr_tail}")]
    MiseInstallFailed { exit_code: i32, stderr_tail: String },

    /// `workflow/engine/bootstrap.rs:652` — a per-step worktree init command
    /// (`[commands] worktree_init_commands` entry, or a workspace-override
    /// row) exited non-zero.
    #[error(
        "{step_name} failed (exit code {exit_code}):\nSTDERR:\n{stderr_tail}\nSTDOUT:\n{stdout_tail}"
    )]
    WorktreeInitCommandFailed {
        step_name: String,
        exit_code: i32,
        stderr_tail: String,
        stdout_tail: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_io_error() -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::PermissionDenied, "permission denied")
    }

    #[test]
    fn lock_in_git_error_display() {
        let cases: Vec<(GitError, &str)> = vec![
            (
                GitError::FetchBaseBranchFailed {
                    base: "main".to_string(),
                    stderr: "boom".to_string(),
                },
                "Failed to fetch base branch 'main': boom",
            ),
            (
                GitError::WorktreeCreateFailed {
                    stderr: "boom".to_string(),
                },
                "Failed to create worktree: boom",
            ),
            (
                GitError::DeleteBranchFailed {
                    branch: "feat/foo".to_string(),
                    stderr: "boom".to_string(),
                },
                "Failed to delete branch feat/foo: boom",
            ),
            (
                GitError::WorktreePathInvalidUtf8 {
                    path: PathBuf::from("/tmp/wt"),
                },
                "worktree path is not valid UTF-8: /tmp/wt",
            ),
            (
                GitError::WorktreeRemoveFailed {
                    stderr: "boom".to_string(),
                },
                "Failed to remove worktree: boom",
            ),
            (
                GitError::WorktreeDirRemoveFailed {
                    path: PathBuf::from("/tmp/wt"),
                    io: sample_io_error(),
                    git_err: "tree corrupt".to_string(),
                },
                "Could not remove stale worktree directory /tmp/wt: permission denied (after git error: tree corrupt)",
            ),
            (
                GitError::GhApiUserFailed {
                    stderr: "boom".to_string(),
                },
                "gh api user failed: boom",
            ),
            (
                GitError::GhApiUserParseJson {
                    source: serde_json::from_str::<serde_json::Value>("{invalid").unwrap_err(),
                },
                // Display delegates to the inner serde_json::Error
                "failed to parse gh api user JSON: key must be a string at line 1 column 2",
            ),
            (
                GitError::GhApiUserEmptyLogin,
                "gh api user returned an empty login",
            ),
            (GitError::EmptyPrUrl, "empty PR URL"),
            (
                GitError::GhPrAddReviewerFailed {
                    stderr: "boom".to_string(),
                },
                "gh pr edit --add-reviewer failed: boom",
            ),
            (
                GitError::GitConfigFailed {
                    setting: "name",
                    stderr: "boom".to_string(),
                },
                "git config user.name failed: boom",
            ),
            (
                GitError::MiseInstallFailed {
                    exit_code: 1,
                    stderr_tail: "tail".to_string(),
                },
                "mise install failed (exit code 1):\ntail",
            ),
            (
                GitError::WorktreeInitCommandFailed {
                    step_name: "Worktree init (1/2): npm ci".to_string(),
                    exit_code: 1,
                    stderr_tail: "err".to_string(),
                    stdout_tail: "out".to_string(),
                },
                "Worktree init (1/2): npm ci failed (exit code 1):\nSTDERR:\nerr\nSTDOUT:\nout",
            ),
        ];
        // Drift detection: bump cases.len() when a new variant lands.
        assert_eq!(cases.len(), 14);
        for (err, expected) in cases {
            assert_eq!(format!("{err}"), expected, "Display mismatch for {err:?}");
        }
    }

    #[test]
    fn lock_in_git_error_into_maestro_error() {
        use crate::error::MaestroError;
        let cases: Vec<GitError> = vec![
            GitError::FetchBaseBranchFailed {
                base: "main".to_string(),
                stderr: "".to_string(),
            },
            GitError::WorktreeCreateFailed {
                stderr: "".to_string(),
            },
            GitError::DeleteBranchFailed {
                branch: "f".to_string(),
                stderr: "".to_string(),
            },
            GitError::WorktreePathInvalidUtf8 {
                path: PathBuf::from("/tmp"),
            },
            GitError::WorktreeRemoveFailed {
                stderr: "".to_string(),
            },
            GitError::WorktreeDirRemoveFailed {
                path: PathBuf::from("/tmp"),
                io: sample_io_error(),
                git_err: "".to_string(),
            },
            GitError::GhApiUserFailed {
                stderr: "".to_string(),
            },
            GitError::GhApiUserParseJson {
                source: serde_json::from_str::<serde_json::Value>("{invalid").unwrap_err(),
            },
            GitError::GhApiUserEmptyLogin,
            GitError::EmptyPrUrl,
            GitError::GhPrAddReviewerFailed {
                stderr: "".to_string(),
            },
            GitError::GitConfigFailed {
                setting: "name",
                stderr: "".to_string(),
            },
            GitError::MiseInstallFailed {
                exit_code: 1,
                stderr_tail: "".to_string(),
            },
            GitError::WorktreeInitCommandFailed {
                step_name: "s".to_string(),
                exit_code: 1,
                stderr_tail: "".to_string(),
                stdout_tail: "".to_string(),
            },
        ];
        assert_eq!(cases.len(), 14);
        for err in cases {
            let outer: MaestroError = err.into();
            assert!(
                matches!(outer, MaestroError::Git(_)),
                "expected MaestroError::Git, got {outer:?}"
            );
        }
    }
}
