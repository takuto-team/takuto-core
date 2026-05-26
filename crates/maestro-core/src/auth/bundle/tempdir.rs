// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Per-workflow secrets-directory lifecycle: creation under
//! `<data_dir>/runtime/secrets/` and best-effort orphan sweep at boot.

use std::path::Path;

use tempfile::TempDir;

use crate::config::ConfigError;
use crate::error::Result;

use super::types::SECRETS_DIR_REL;

/// Task #43: resolve the host-side directory in which per-workflow secret
/// tempdirs live, and create a fresh TempDir inside it.
///
/// Lives at `<data_dir>/runtime/secrets/<random>` when `data_dir` is
/// available; falls back to the process tempdir when it isn't (in-memory
/// test DB). The fallback path is fine for unit tests — they never bind
/// the dir into a DinD container.
pub(super) fn secrets_dir_for_db(db: &crate::db::Database) -> Result<TempDir> {
    if let Some(data_dir) = db.data_dir() {
        let root = data_dir.join(SECRETS_DIR_REL);
        std::fs::create_dir_all(&root).map_err(|e| ConfigError::BundleSecretFile {
            op: "create-root",
            path: root.clone(),
            detail: e.to_string(),
        })?;
        tempfile::Builder::new()
            .prefix("bundle-")
            .tempdir_in(&root)
            .map_err(|source| ConfigError::BundleTempdir { source }.into())
    } else {
        // No data_dir → in-memory DB → unit test path. Fall back to
        // process tempdir (`/tmp/...`). Tests never reach the bind-mount
        // resolver so this is safe.
        TempDir::new().map_err(|source| ConfigError::BundleTempdir { source }.into())
    }
}

/// Task #43: best-effort startup sweep. `<data_dir>/runtime/secrets/`
/// accumulates orphan bundle dirs when maestro crashes between TempDir
/// creation and drop. Call this once at process boot — every entry is a
/// dead bundle dir from a previous run (the current run hasn't created
/// any yet). Logs at info level so operators see the cleanup happen.
pub fn cleanup_orphan_secrets(data_dir: &Path) -> std::io::Result<usize> {
    let root = data_dir.join(SECRETS_DIR_REL);
    if !root.exists() {
        return Ok(0);
    }
    let mut swept = 0_usize;
    for entry in std::fs::read_dir(&root)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            if let Err(e) = std::fs::remove_dir_all(&path) {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "failed to remove orphan secrets dir; will retry on next boot"
                );
                continue;
            }
            swept += 1;
        }
    }
    if swept > 0 {
        tracing::info!(
            data_dir = %data_dir.display(),
            count = swept,
            "Swept orphan WorkerSecretsBundle directories from a prior run"
        );
    }
    Ok(swept)
}
