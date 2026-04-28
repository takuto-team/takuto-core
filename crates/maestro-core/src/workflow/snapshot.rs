// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::collections::HashMap;
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
    /// Direct URL to the ticket in the ticketing system (e.g. GitHub issue HTML URL).
    /// `None` for Jira workflows and old snapshots without this field.
    #[serde(default)]
    pub ticket_url: Option<String>,
    /// Whether the workflow driver was spawned. Old snapshots without this field
    /// default to `true` (driver was running).
    #[serde(default = "default_driver_started")]
    pub driver_started: bool,
    /// Status of each dynamic workflow definition run for this ticket.
    #[serde(default)]
    pub workflow_def_runs: HashMap<String, crate::workflow::definitions::WorkflowDefRunState>,
    /// `true` once the full bootstrap (mise install + hooks) has completed for this workflow.
    /// When `false`, the next workflow-def start must run bootstrap even if a worktree already
    /// exists (worktree was pre-created at ticket-add time but setup has not run yet).
    #[serde(default)]
    pub worktree_bootstrapped: bool,
}

fn default_jira_available() -> bool {
    true
}

fn default_driver_started() -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow::state::WorkflowState;

    #[test]
    fn driver_started_round_trips_through_snapshot() {
        let rec = PersistedWorkflowRecord {
            id: "id".into(),
            ticket_key: "X-1".into(),
            ticket_summary: "s".into(),
            ticket_description: String::new(),
            ticket_type: "Task".into(),
            state: WorkflowState::Pending,
            started_at: Utc::now(),
            updated_at: Utc::now(),
            steps_log: vec![],
            branch_name: String::new(),
            worktree_path: None,
            pr_url: None,
            pr_merged: false,
            terminal_lines: vec![],
            current_step_label: None,
            started_manually: true,
            jira_available: true,
            last_session_id: None,
            description_session_id: None,
            ticketing_system: crate::config::TicketingSystem::None,
            ticket_url: None,
            driver_started: false,
            workflow_def_runs: HashMap::new(),
            worktree_bootstrapped: false,
        };
        let json = serde_json::to_string(&rec).unwrap();
        let back: PersistedWorkflowRecord = serde_json::from_str(&json).unwrap();
        assert!(!back.driver_started);
    }

    #[test]
    fn missing_driver_started_defaults_to_true() {
        // Simulate an old snapshot without driver_started field
        let json = r#"{
            "id": "id",
            "ticket_key": "X-1",
            "ticket_summary": "s",
            "ticket_description": "",
            "ticket_type": "Task",
            "state": "Pending",
            "started_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
            "steps_log": [],
            "branch_name": "",
            "worktree_path": null,
            "pr_url": null,
            "pr_merged": false,
            "terminal_lines": [],
            "current_step_label": null,
            "started_manually": true,
            "jira_available": true,
            "last_session_id": null,
            "description_session_id": null
        }"#;
        let rec: PersistedWorkflowRecord = serde_json::from_str(json).unwrap();
        assert!(
            rec.driver_started,
            "old snapshots must default driver_started to true"
        );
    }
}
