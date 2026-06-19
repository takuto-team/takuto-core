// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Resolution helpers: `workspace_name`, repo/branch lookup, worktree init
//! command lookup, and workflow-definitions directory scanning.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::warn;

use crate::config::Config;
use crate::db::Database;

use super::types::Workflow;

/// Resolve a workflow's `workspace_name` (denormalised, used for snapshot
/// grouping + dashboard back-compat). Mirrors `resolve_repo_for_ticket` but
/// runs at workflow-creation time when the ticket is not yet in the map.
///
/// - `repository_id` set + DB available → look up `repositories.name`.
/// - Otherwise → fall back to `cfg.git.repo_path`'s last path component
///   (legacy behaviour, kept so unit tests with no DB still produce a sane
///   workspace_name).
pub(crate) async fn resolve_workspace_name(
    repository_id: Option<&str>,
    db: Option<&Database>,
    config: &Arc<RwLock<Config>>,
) -> String {
    if let (Some(repo_id), Some(database)) = (repository_id, db)
        && let Ok(Some(row)) = crate::db::repositories::get(database.adapter(), repo_id).await
    {
        return row.name;
    }
    let cfg = config.read().await;
    crate::workflow::snapshot::workspace_name_from_repo_path(std::path::Path::new(
        &cfg.git.repo_path,
    ))
}

/// Resolve `(repo_path, default_branch)` for a workflow.
///
/// Threading rule: every workflow stores `repository_id` (the durable FK
/// to the `repositories` row) plus `workspace_name` (denormalised
/// back-compat handle). This helper looks them up in priority order:
///   1. `repository_id → db::repositories::get` (canonical).
///   2. `workspace_name → db::repositories::get_by_name` (defensive — covers
///      restored snapshots not yet back-filled by `migrate_orphan_repo_associations`).
///   3. Fallback: `cfg.git.repo_path` + `cfg.git.base_branch` — only useful for
///      tests/dry-mode paths where no DB is attached. Production callers always
///      have a DB and a registered repository.
pub(crate) async fn resolve_repo_for_ticket(
    ticket_key: &str,
    workflows: &Arc<RwLock<HashMap<String, Workflow>>>,
    config: &Arc<RwLock<Config>>,
    db: Option<&Database>,
) -> (PathBuf, String) {
    let (repository_id, workspace_name) = {
        let wf = workflows.read().await;
        wf.get(ticket_key)
            .map(|w| (w.repository_id.clone(), w.workspace_name.clone()))
            .unwrap_or_default()
    };

    if let (Some(repo_id), Some(database)) = (repository_id.as_deref(), db) {
        match crate::db::repositories::get(database.adapter(), repo_id).await {
            Ok(Some(row)) => return (PathBuf::from(&row.local_path), row.default_branch),
            Ok(None) => {
                warn!(
                    repository_id = repo_id,
                    ticket = ticket_key,
                    "Workflow.repository_id missing from `repositories` table; falling back"
                );
            }
            Err(e) => {
                warn!(error = %e, "DB lookup for Workflow.repository_id failed; falling back");
            }
        }
    }

    if let Some(database) = db
        && !workspace_name.is_empty()
        && let Ok(Some(row)) =
            crate::db::repositories::get_by_name(database.adapter(), &workspace_name).await
    {
        return (PathBuf::from(&row.local_path), row.default_branch);
    }

    let cfg = config.read().await;
    (
        PathBuf::from(&cfg.git.repo_path),
        cfg.git.base_branch.clone(),
    )
}

/// Scan the definitions directory and return a sorted list of `(filename, modified_time)` tuples
/// for change detection.
pub(super) fn scan_definitions_dir(dir: &Path) -> Vec<(String, std::time::SystemTime)> {
    let mut entries = Vec::new();
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return entries;
    };
    for entry in read_dir.flatten() {
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str());
        if (ext == Some("yml") || ext == Some("yaml"))
            && let Ok(meta) = path.metadata()
        {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            if !name.is_empty() {
                entries.push((name, meta.modified().unwrap_or(std::time::UNIX_EPOCH)));
            }
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries
}

/// Resolve the list of `worktree_init_commands` for the workflow owner's
/// `(user_id, workspace_name)` pair.
///
/// Resolution rules:
/// * If `workflow_user_id` is `None` or no `db` is available (e.g. some test
///   paths), return an empty vec — the bootstrap runs no init commands.
/// * Otherwise, look up `user_worktree_commands` for the owner; if a row
///   exists, return its `init_commands`. No row → empty vec.
///
/// There is no global-default fallback; init commands are exclusively a
/// per-user-per-workspace concern (configured from the dashboard's Worktree
/// Settings tab).
///
/// Exposed at crate level (not pub(super)) so integration tests can call it
/// directly without needing to spin up a real Docker driver.
pub async fn resolve_worktree_init_commands(
    workflow_user_id: Option<&str>,
    workspace_name: &str,
    db: Option<&Database>,
) -> Vec<String> {
    let (Some(user_id), Some(db)) = (workflow_user_id, db) else {
        return Vec::new();
    };
    // user_worktree_commands uses the agnostic adapter; no rusqlite
    // MutexGuard needed. The DAO is async — direct call from this
    // already-async fn.
    match crate::db::user_worktree_commands::get(db.adapter(), user_id, workspace_name).await {
        Ok(Some(row)) => row.init_commands,
        Ok(None) => Vec::new(),
        Err(e) => {
            warn!(
                user_id = %user_id,
                workspace = %workspace_name,
                error = %e,
                "Failed to read user_worktree_commands; running zero init commands"
            );
            Vec::new()
        }
    }
}

/// Resolve the per-`(user, workspace)` report toggle (`generate_report`).
///
/// Same resolution rules as [`resolve_worktree_init_commands`]: `false` when
/// there's no `user_id` / no `db` / no row / a read error. This is the
/// source of truth for whether workflow flow runs generate a per-flow report
/// (the legacy global `[general] generate_report` is no longer consulted by
/// the engine).
pub async fn resolve_worktree_generate_report(
    workflow_user_id: Option<&str>,
    workspace_name: &str,
    db: Option<&Database>,
) -> bool {
    let (Some(user_id), Some(db)) = (workflow_user_id, db) else {
        return false;
    };
    match crate::db::user_worktree_commands::get(db.adapter(), user_id, workspace_name).await {
        Ok(Some(row)) => row.generate_report,
        Ok(None) => false,
        Err(e) => {
            warn!(
                user_id = %user_id,
                workspace = %workspace_name,
                error = %e,
                "Failed to read user_worktree_commands; report generation off"
            );
            false
        }
    }
}
