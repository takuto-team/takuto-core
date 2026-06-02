// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Startup reconciliation helpers.
//!
//! Two passes run at startup, BEFORE `engine.restore_persisted_workflows()`:
//!
//! 1. **Filesystem → `repositories`**: for every `<dir>/.git` directory under
//!    `WORKSPACES_DIR`, ensure a row exists in `repositories` with the
//!    on-disk path, the discovered remote URL, and the discovered default
//!    branch. Atomic against concurrent first-boot via `INSERT … ON CONFLICT
//!    DO NOTHING`. Idempotent.
//!
//! 2. **Snapshot → `user_repositories`**: for every persisted workflow with a
//!    known `user_id` and a `workspace_name` matching a registered repo,
//!    ensure the `(user_id, repository_id)` association exists. Without this
//!    backfill, restored workflows would disappear from their owner's
//!    dashboard once user-scoped filtering is enforced.
//!
//! The reconciliation is the *only* place we read snapshot files without
//! consuming them into the engine — we intentionally use
//! `workflow::snapshot::read_all_workspace_snapshots` which doesn't mutate
//! state.
//!
//! Both helpers are `async fn` taking `&DbAdapter`; callers await them
//! directly.

use std::path::Path;

use crate::db::DbAdapter;
use crate::error::Result;

/// Scan `workspaces_dir` for `<dir>/.git` directories and register each as a
/// row in `repositories`. Returns the number of new rows inserted on this run
/// (rows that already existed are not counted).
///
/// Behaviour:
/// - Missing `workspaces_dir` → silently returns 0 (fresh deployments).
/// - Per-repo URL discovery: parses `.git/config` (or `.git/HEAD` + `.git/...`
///   when `.git` is a file/gitlink — uncommon; treated as None).
/// - Per-repo `default_branch` discovery: `git symbolic-ref
///   refs/remotes/origin/HEAD` shelled out; on error falls back to `"main"`.
/// - Failures on individual repos are logged at `warn` and don't halt the
///   scan.
///
/// The caller owns the SQLite lock and passes a borrowed `Connection` so the
/// reconciliation runs in a single transaction-equivalent window with no
/// async cross-task locking concerns.
pub async fn reconcile_repositories(adapter: &DbAdapter, workspaces_dir: &str) -> Result<usize> {
    let path = Path::new(workspaces_dir);
    if !path.exists() {
        return Ok(0);
    }

    let entries = match std::fs::read_dir(path) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(
                workspaces_dir = %workspaces_dir,
                error = %e,
                "Cannot read WORKSPACES_DIR for reconciliation; skipping"
            );
            return Ok(0);
        }
    };

    let mut inserted = 0usize;
    for entry in entries.filter_map(|e| e.ok()) {
        let repo_path = entry.path();
        if !repo_path.join(".git").exists() {
            continue;
        }
        let name = match repo_path.file_name() {
            Some(n) => n.to_string_lossy().into_owned(),
            None => continue,
        };
        let local_path = repo_path.to_string_lossy().into_owned();
        let repo_url = read_git_remote_url(&repo_path);
        let default_branch = read_default_branch(&repo_path).unwrap_or_else(|| "main".to_string());

        // Was this repo already registered? Compare before/after upsert.
        let pre = crate::db::repositories::get_by_path(adapter, &local_path).await?;
        match crate::db::repositories::upsert(
            adapter,
            &name,
            repo_url.as_deref(),
            &local_path,
            &default_branch,
            None,
        )
        .await
        {
            Ok(_id) => {
                if pre.is_none() {
                    inserted += 1;
                    tracing::info!(
                        name = %name,
                        local_path = %local_path,
                        repo_url = ?repo_url,
                        default_branch = %default_branch,
                        "Reconciliation: registered repository"
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    name = %name,
                    local_path = %local_path,
                    error = %e,
                    "Reconciliation: failed to upsert repository"
                );
            }
        }
    }

    Ok(inserted)
}

/// For every restored snapshot workflow with `user_id == Some(uid)` and a
/// `workspace_name` matching a registered repository, insert
/// `user_repositories(uid, repository_id, now) ON CONFLICT DO NOTHING`.
/// Returns the number of new associations inserted on this run.
///
/// Reads snapshots without consuming them — the engine restore path runs
/// separately afterwards.
pub async fn backfill_user_repositories_from_snapshots(
    adapter: &DbAdapter,
    data_dir: &Path,
) -> Result<usize> {
    let records = match crate::workflow::snapshot::read_all_workspace_snapshots(data_dir) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "Backfill: failed to read workspace snapshots");
            return Ok(0);
        }
    };

    let mut backfilled = 0usize;
    for rec in records {
        let Some(uid) = rec.user_id.as_ref() else {
            continue;
        };
        if rec.workspace_name.is_empty() {
            continue;
        }
        let Some(repo) = crate::db::repositories::get_by_name(adapter, &rec.workspace_name).await?
        else {
            // The workflow points at a workspace_name we couldn't reconcile to
            // a registered repository — skip silently. The workflow will be
            // invisible until an admin re-adds the repo.
            continue;
        };
        if crate::db::repositories::add_for_user(adapter, uid, &repo.id).await? {
            backfilled += 1;
        }
    }

    Ok(backfilled)
}

/// Read the `origin` remote URL from `.git/config` and normalise to an HTTPS
/// GitHub URL (`git@…:owner/repo.git` → `https://github.com/owner/repo`).
///
/// Mirrors the helper in `maestro_web::routes::repos::read_git_remote_url`
/// which is `pub(super)` and therefore unreachable from here. Kept identical
/// in behaviour; both copies should eventually reduce to one `pub` helper.
pub fn read_git_remote_url(repo_path: &Path) -> Option<String> {
    let git_config = std::fs::read_to_string(repo_path.join(".git/config")).ok()?;
    let mut in_origin = false;
    for line in git_config.lines() {
        let trimmed = line.trim();
        if trimmed == r#"[remote "origin"]"# {
            in_origin = true;
            continue;
        }
        if in_origin {
            if trimmed.starts_with('[') {
                break;
            }
            if let Some(rest) = trimmed.strip_prefix("url =") {
                return Some(normalize_github_url(rest.trim()));
            }
        }
    }
    None
}

fn normalize_github_url(url: &str) -> String {
    if let Some(path) = url.strip_prefix("git@github.com:") {
        return format!("https://github.com/{}", path.trim_end_matches(".git"));
    }
    if let Some(path) = url.strip_prefix("ssh://git@github.com/") {
        return format!("https://github.com/{}", path.trim_end_matches(".git"));
    }
    url.trim_end_matches(".git").to_string()
}

/// Discover the default branch from `git symbolic-ref refs/remotes/origin/HEAD`.
/// Falls back to `None` on any error (caller substitutes `"main"`).
fn read_default_branch(repo_path: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["symbolic-ref", "refs/remotes/origin/HEAD"])
        .current_dir(repo_path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    // Output looks like "refs/remotes/origin/main\n" — strip the prefix.
    let trimmed = stdout.trim();
    trimmed
        .strip_prefix("refs/remotes/origin/")
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_ssh_git_at_url() {
        assert_eq!(
            normalize_github_url("git@github.com:owner/repo.git"),
            "https://github.com/owner/repo"
        );
    }

    #[test]
    fn normalize_https_url_with_git_suffix() {
        assert_eq!(
            normalize_github_url("https://github.com/owner/repo.git"),
            "https://github.com/owner/repo"
        );
    }

    #[test]
    fn read_git_remote_url_missing_repo_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(read_git_remote_url(tmp.path()).is_none());
    }

    #[test]
    fn read_git_remote_url_parses_origin() {
        let tmp = tempfile::tempdir().unwrap();
        let git_dir = tmp.path().join(".git");
        std::fs::create_dir(&git_dir).unwrap();
        std::fs::write(
            git_dir.join("config"),
            r#"[core]
    repositoryformatversion = 0
[remote "origin"]
    url = git@github.com:owner/repo.git
    fetch = +refs/heads/*:refs/remotes/origin/*
"#,
        )
        .unwrap();
        assert_eq!(
            read_git_remote_url(tmp.path()).as_deref(),
            Some("https://github.com/owner/repo")
        );
    }
}
