// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::state::WorkflowState;
use super::step::StepLog;

/// File name for the workflow snapshot.
pub const SNAPSHOT_FILENAME: &str = "workflow_snapshot.json";

pub const SNAPSHOT_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
pub struct WorkflowSnapshotFile {
    pub version: u32,
    pub workflows: Vec<PersistedWorkflowRecord>,
}

/// Serializable workflow row for container restart / resume.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedWorkflowRecord {
    pub id: String,
    pub ticket_key: String,
    pub ticket_summary: String,
    pub ticket_description: String,
    pub ticket_type: String,
    pub state: WorkflowState,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub steps_log: Vec<StepLog>,
    pub branch_name: String,
    pub worktree_path: Option<PathBuf>,
    pub pr_url: Option<String>,
    #[serde(default)]
    pub pr_merged: bool,
    pub terminal_lines: Vec<PersistedTerminalLine>,
    pub current_step_label: Option<String>,
    /// Dashboard **Start workflow** (manual); poller-started workflows omit this field (deserializes as `false`).
    #[serde(default)]
    pub started_manually: bool,
    /// `true` when Jira (acli) was available at workflow creation. Older snapshots omit this field
    /// and deserialize as `true` (backward-compatible default).
    #[serde(default = "default_jira_available")]
    pub jira_available: bool,
    /// Last Claude/Cursor session ID for `--resume` across restarts.
    #[serde(default)]
    pub last_session_id: Option<String>,
    /// Persistent session ID shared by "Improve with AI" and "Ask AI" for this workflow.
    #[serde(default)]
    pub description_session_id: Option<String>,
    /// Ticketing system active when this workflow was created. `#[serde(default)]` means
    /// old snapshots without this field get `TicketingSystem::None`.
    #[serde(default)]
    pub ticketing_system: crate::config::TicketingSystem,
}

fn default_jira_available() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedTerminalLine {
    pub text: String,
    pub stream: String,
}

/// Resolve the directory for storing the workflow snapshot.
///
/// Priority:
/// 1. `$MAESTRO_DATA_DIR` — explicit override
/// 2. `$MAESTRO_HOME/.maestro` — container convention (MAESTRO_HOME=/home/maestro)
/// 3. `$HOME/.maestro` — local dev fallback
/// 4. `repo_path/.maestro` — legacy fallback
pub fn resolve_snapshot_dir(repo_path: &Path) -> PathBuf {
    if let Ok(dir) = std::env::var("MAESTRO_DATA_DIR")
        && !dir.is_empty()
    {
        return PathBuf::from(dir);
    }
    if let Ok(home) = std::env::var("MAESTRO_HOME")
        && !home.is_empty()
    {
        return PathBuf::from(home).join(".maestro");
    }
    if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
    {
        return PathBuf::from(home).join(".maestro");
    }
    // Legacy fallback — inside the repo
    repo_path.join(".maestro")
}

pub fn snapshot_path(repo_path: &Path) -> PathBuf {
    resolve_snapshot_dir(repo_path).join(SNAPSHOT_FILENAME)
}

/// Legacy snapshot path inside `{repo_path}/.maestro/`.
fn legacy_snapshot_path(repo_path: &Path) -> PathBuf {
    repo_path.join(".maestro").join(SNAPSHOT_FILENAME)
}

pub fn write_workflow_snapshot(
    repo_path: &Path,
    workflows: &[PersistedWorkflowRecord],
) -> crate::error::Result<()> {
    let dir = resolve_snapshot_dir(repo_path);
    fs::create_dir_all(&dir)?;
    let path = dir.join(SNAPSHOT_FILENAME);
    let tmp = path.with_extension("json.tmp");
    let file = WorkflowSnapshotFile {
        version: SNAPSHOT_VERSION,
        workflows: workflows.to_vec(),
    };
    let json = serde_json::to_string_pretty(&file)
        .map_err(|e| crate::error::MaestroError::Config(e.to_string()))?;
    fs::write(&tmp, json)?;
    fs::rename(&tmp, &path)?;
    Ok(())
}

/// Read the workflow snapshot, checking the new location first then migrating from the legacy
/// `{repo_path}/.maestro/` location if needed.
pub fn read_workflow_snapshot(
    repo_path: &Path,
) -> crate::error::Result<Option<WorkflowSnapshotFile>> {
    let path = snapshot_path(repo_path);
    if path.exists() {
        let bytes = fs::read(&path)?;
        let file: WorkflowSnapshotFile = serde_json::from_slice(&bytes)
            .map_err(|e| crate::error::MaestroError::Config(e.to_string()))?;
        return Ok(Some(file));
    }

    // Try the legacy location and migrate if found.
    let legacy = legacy_snapshot_path(repo_path);
    if legacy != path && legacy.exists() {
        let bytes = fs::read(&legacy)?;
        let file: WorkflowSnapshotFile = serde_json::from_slice(&bytes)
            .map_err(|e| crate::error::MaestroError::Config(e.to_string()))?;
        tracing::info!(
            from = %legacy.display(),
            to = %path.display(),
            "Migrating workflow snapshot from legacy location"
        );
        // Best-effort migration: copy to the new location and remove the old one.
        // If the new location is not writable (e.g. volume permissions), still return
        // the snapshot so workflows are restored — migration will succeed on next write.
        let dir = resolve_snapshot_dir(repo_path);
        match fs::create_dir_all(&dir).and_then(|_| fs::write(&path, &bytes)) {
            Ok(()) => {
                let _ = fs::remove_file(&legacy);
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Could not write snapshot to new location; using legacy path (migration will retry)"
                );
            }
        }
        return Ok(Some(file));
    }

    Ok(None)
}

pub fn remove_workflow_snapshot(repo_path: &Path) -> crate::error::Result<()> {
    let path = snapshot_path(repo_path);
    if path.exists() {
        fs::remove_file(&path)?;
    }
    Ok(())
}
