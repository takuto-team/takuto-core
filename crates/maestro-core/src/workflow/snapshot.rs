use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::state::WorkflowState;
use super::step::StepLog;

/// File name under `{git.repo_path}/.maestro/`.
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
    pub terminal_lines: Vec<PersistedTerminalLine>,
    pub current_step_label: Option<String>,
    /// Dashboard **Start workflow** (manual); poller-started workflows omit this field (deserializes as `false`).
    #[serde(default)]
    pub started_manually: bool,
    /// `true` when Jira (acli) was available at workflow creation. Older snapshots omit this field
    /// and deserialize as `true` (backward-compatible default).
    #[serde(default = "default_jira_available")]
    pub jira_available: bool,
}

fn default_jira_available() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedTerminalLine {
    pub text: String,
    pub stream: String,
}

pub fn snapshot_path(repo_path: &Path) -> PathBuf {
    repo_path.join(".maestro").join(SNAPSHOT_FILENAME)
}

pub fn write_workflow_snapshot(
    repo_path: &Path,
    workflows: &[PersistedWorkflowRecord],
) -> crate::error::Result<()> {
    let dir = repo_path.join(".maestro");
    fs::create_dir_all(&dir)?;
    let path = snapshot_path(repo_path);
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

pub fn read_workflow_snapshot(
    repo_path: &Path,
) -> crate::error::Result<Option<WorkflowSnapshotFile>> {
    let path = snapshot_path(repo_path);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(&path)?;
    let file: WorkflowSnapshotFile = serde_json::from_slice(&bytes)
        .map_err(|e| crate::error::MaestroError::Config(e.to_string()))?;
    Ok(Some(file))
}

pub fn remove_workflow_snapshot(repo_path: &Path) -> crate::error::Result<()> {
    let path = snapshot_path(repo_path);
    if path.exists() {
        fs::remove_file(&path)?;
    }
    Ok(())
}
