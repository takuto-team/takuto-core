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
    /// Name of the workspace (repo directory name) this workflow belongs to.
    /// Old snapshots without this field get an empty string (assigned during restore).
    #[serde(default)]
    pub workspace_name: String,
    /// FK to `repositories.id`. Old snapshots that lack the field deserialize as
    /// `None`. The startup reconciliation (`migrate_orphan_repo_associations`)
    /// back-fills it by joining `workspace_name` against `repositories.name`.
    /// Workflows that cannot be back-filled stay `None` and are hidden from
    /// the dashboard.
    #[serde(default)]
    pub repository_id: Option<String>,
    /// ID of the user who created this workflow. Old snapshots without this field
    /// get `None` (unowned — visible to admins only during migration).
    #[serde(default)]
    pub user_id: Option<String>,
    /// Credentials pinned at the workflow's first agent step. Survives an
    /// admin provider switch mid-flight — the workflow keeps using the
    /// credential row id it was pinned to, while the row's `inactive`
    /// flag may have flipped to `1` for new workflows.
    /// `#[serde(default)]` so older snapshots (which predate the
    /// pinning mechanism) deserialize as `None`.
    #[serde(default)]
    pub auth_pin: Option<AuthPin>,
}

/// Credentials pinned at workflow start. Survives an admin provider switch
/// mid-flight per 04_architecture.md §7.2.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthPin {
    /// Active AI provider when the workflow started (`"claude" | "cursor" |
    /// "codex" | "opencode"`).
    pub provider: String,
    /// `user_provider_credentials.id` for the pinned row. `None` when the
    /// workflow ran against a deployment-default credential (env var
    /// fallback; only valid when the provider's `allow_shared_default = true`).
    #[serde(default)]
    pub provider_credential_row_id: Option<i64>,
    /// `"app" | "user_pat"` — which side of the §4.2 mode matrix the
    /// resolver was in when the pin landed.
    pub github_mode: String,
    /// Stable identifier of the GitHub credential row (when PAT-mode).
    /// `None` for App-mode pins.
    #[serde(default)]
    pub github_credential_row_id: Option<i64>,
    /// RFC-3339 timestamp.
    pub started_at: String,
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

/// Extract the workspace name (repo directory name) from a repo path.
/// e.g. `/workspaces/my-repo` → `"my-repo"`, `/workspace` → `"workspace"`.
pub fn workspace_name_from_repo_path(repo_path: &Path) -> String {
    repo_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "default".to_string())
}

/// Resolve the base data directory for storing workflow snapshots.
///
/// Priority:
/// 1. `$TAKUTO_DATA_DIR` — explicit override
/// 2. `$TAKUTO_HOME/.takuto` — container convention (TAKUTO_HOME=/home/takuto)
/// 3. `$HOME/.takuto` — local dev fallback
/// 4. `repo_path/.takuto` — legacy fallback
pub fn resolve_snapshot_dir(repo_path: &Path) -> PathBuf {
    if let Some(dir) = resolve_data_dir() {
        return dir;
    }
    // Legacy fallback — inside the repo
    repo_path.join(".takuto")
}

const ACTIVE_WORKSPACE_FILE: &str = "active_workspace";

/// Well-known base directory for project repositories (Docker / devcontainer convention).
pub const WORKSPACES_DIR: &str = "/workspaces";

/// Resolve the data directory without needing a repo_path (uses env vars only,
/// falls back to `$HOME/.takuto`). Returns `None` only when no env var is set.
pub fn resolve_data_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("TAKUTO_DATA_DIR")
        && !dir.is_empty()
    {
        return Some(PathBuf::from(dir));
    }
    if let Ok(home) = std::env::var("TAKUTO_HOME")
        && !home.is_empty()
    {
        return Some(PathBuf::from(home).join(".takuto"));
    }
    if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
    {
        return Some(PathBuf::from(home).join(".takuto"));
    }
    None
}

/// Read the active workspace name from `{data_dir}/active_workspace`.
/// Returns `None` if the file doesn't exist or is empty.
pub fn read_active_workspace() -> Option<String> {
    let data_dir = resolve_data_dir()?;
    let path = data_dir.join(ACTIVE_WORKSPACE_FILE);
    let content = std::fs::read_to_string(&path).ok()?;
    let name = content.trim().to_string();
    if name.is_empty() { None } else { Some(name) }
}

/// Write the active workspace name to `{data_dir}/active_workspace`.
pub fn write_active_workspace(name: &str) -> std::io::Result<()> {
    let data_dir = resolve_data_dir().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "no data directory available")
    })?;
    std::fs::create_dir_all(&data_dir)?;
    std::fs::write(data_dir.join(ACTIVE_WORKSPACE_FILE), name.trim())?;
    Ok(())
}

/// Resolve the repo path for the active workspace. Reads the persisted active
/// workspace name, falls back to scanning `/workspaces/` for the most recently
/// modified repo, and returns `/workspaces/{name}` if found.
pub fn resolve_active_repo_path() -> Option<String> {
    // 1. Try the persisted active workspace
    if let Some(name) = read_active_workspace() {
        // Reject path traversal in persisted workspace name.
        if name.contains('/') || name.contains("..") || name.starts_with('.') {
            tracing::warn!(name = %name, "Ignoring persisted active workspace: invalid name");
        } else {
            let p = Path::new(WORKSPACES_DIR).join(&name);
            if p.join(".git").exists() {
                return Some(p.to_string_lossy().into_owned());
            }
        }
    }
    // 2. Fall back to scanning /workspaces/ for any repo
    let entries = std::fs::read_dir(WORKSPACES_DIR).ok()?;
    let candidate = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join(".git").exists())
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())?;
    let path = candidate.path().to_string_lossy().into_owned();
    tracing::warn!(
        selected = %path,
        "No persisted active workspace found — auto-selecting most recently modified repo"
    );
    // Persist the auto-selected workspace for next startup
    if let Some(name) = candidate.path().file_name() {
        let _ = write_active_workspace(&name.to_string_lossy());
    }
    Some(path)
}

/// Per-workspace snapshot directory: `{data_dir}/workspaces/{workspace_name}/`.
pub fn resolve_workspace_snapshot_dir(repo_path: &Path) -> PathBuf {
    let data_dir = resolve_snapshot_dir(repo_path);
    let ws_name = workspace_name_from_repo_path(repo_path);
    data_dir.join("workspaces").join(ws_name)
}

pub fn snapshot_path(repo_path: &Path) -> PathBuf {
    resolve_workspace_snapshot_dir(repo_path).join(SNAPSHOT_FILENAME)
}

/// Scan every per-workspace snapshot
/// under `{data_dir}/workspaces/*/workflow_snapshot.json` and return the
/// `ticket_key` of each workflow that is currently in flight. "In flight"
/// means **not** in a terminal state (`Done` / `Stopped` / `Error`) and
/// **not** `Paused` — i.e. the workflow has a live driver task that would
/// lose its sealed credentials mid-step if the master key were reset.
///
/// Used by `takuto keys reset` as the safety gate; surfaces the active
/// ticket keys so the operator knows which workflows must finish or be
/// stopped before retrying.
///
/// Returns `Ok(vec![])` when `{data_dir}/workspaces` does not exist (fresh
/// install). Returns `Err` only on real I/O failure during directory
/// traversal; malformed individual snapshot files are skipped silently
/// because we'd rather under-report than block a reset on garbage data.
pub fn scan_in_flight_workflow_keys(data_dir: &Path) -> std::io::Result<Vec<String>> {
    use crate::workflow::state::WorkflowState;

    let workspaces_root = data_dir.join("workspaces");
    if !workspaces_root.is_dir() {
        return Ok(Vec::new());
    }
    let mut keys: Vec<String> = Vec::new();
    for entry in fs::read_dir(&workspaces_root)?.flatten() {
        let snap_path = entry.path().join(SNAPSHOT_FILENAME);
        if !snap_path.is_file() {
            continue;
        }
        let Ok(bytes) = fs::read(&snap_path) else {
            continue;
        };
        let Ok(file) = serde_json::from_slice::<WorkflowSnapshotFile>(&bytes) else {
            continue;
        };
        for rec in file.workflows {
            let in_flight = !matches!(
                rec.state,
                WorkflowState::Done
                    | WorkflowState::Stopped
                    | WorkflowState::Error { .. }
                    | WorkflowState::Paused { .. }
            );
            if in_flight {
                keys.push(rec.ticket_key);
            }
        }
    }
    Ok(keys)
}

/// Legacy snapshot path inside `{repo_path}/.takuto/`.
fn legacy_snapshot_path(repo_path: &Path) -> PathBuf {
    repo_path.join(".takuto").join(SNAPSHOT_FILENAME)
}

pub fn write_workflow_snapshot(
    repo_path: &Path,
    workflows: &[PersistedWorkflowRecord],
) -> crate::error::Result<()> {
    let dir = resolve_workspace_snapshot_dir(repo_path);
    fs::create_dir_all(&dir)?;
    let path = dir.join(SNAPSHOT_FILENAME);
    let tmp = path.with_extension("json.tmp");
    let file = WorkflowSnapshotFile {
        version: SNAPSHOT_VERSION,
        workflows: workflows.to_vec(),
    };
    let json =
        serde_json::to_string_pretty(&file).map_err(|e| crate::config::ConfigError::Snapshot {
            op: "serde_json",
            detail: e.to_string(),
        })?;
    fs::write(&tmp, json)?;
    fs::rename(&tmp, &path)?;
    Ok(())
}

/// Read the workflow snapshot for a single workspace, checking the per-workspace location first,
/// then migrating from legacy locations if needed.
pub fn read_workflow_snapshot(
    repo_path: &Path,
) -> crate::error::Result<Option<WorkflowSnapshotFile>> {
    let path = snapshot_path(repo_path);
    if path.exists() {
        let bytes = fs::read(&path)?;
        let file: WorkflowSnapshotFile =
            serde_json::from_slice(&bytes).map_err(|e| crate::config::ConfigError::Snapshot {
                op: "serde_json",
                detail: e.to_string(),
            })?;
        return Ok(Some(file));
    }

    // Try the legacy in-repo location and migrate if found.
    let legacy = legacy_snapshot_path(repo_path);
    if legacy != path && legacy.exists() {
        let bytes = fs::read(&legacy)?;
        let mut file: WorkflowSnapshotFile =
            serde_json::from_slice(&bytes).map_err(|e| crate::config::ConfigError::Snapshot {
                op: "serde_json",
                detail: e.to_string(),
            })?;
        let ws_name = workspace_name_from_repo_path(repo_path);
        // Backfill workspace_name for legacy records.
        for rec in &mut file.workflows {
            if rec.workspace_name.is_empty() {
                rec.workspace_name = ws_name.clone();
            }
        }
        tracing::info!(
            from = %legacy.display(),
            to = %path.display(),
            "Migrating workflow snapshot from legacy location"
        );
        let dir = resolve_workspace_snapshot_dir(repo_path);
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

    // Try the old global location (pre-workspace-isolation) and migrate.
    let global = resolve_snapshot_dir(repo_path).join(SNAPSHOT_FILENAME);
    if global != path && global.exists() {
        let bytes = fs::read(&global)?;
        let mut file: WorkflowSnapshotFile =
            serde_json::from_slice(&bytes).map_err(|e| crate::config::ConfigError::Snapshot {
                op: "serde_json",
                detail: e.to_string(),
            })?;
        let ws_name = workspace_name_from_repo_path(repo_path);
        for rec in &mut file.workflows {
            if rec.workspace_name.is_empty() {
                rec.workspace_name = ws_name.clone();
            }
        }
        tracing::info!(
            from = %global.display(),
            to = %path.display(),
            "Migrating workflow snapshot from global location to per-workspace"
        );
        let dir = resolve_workspace_snapshot_dir(repo_path);
        match fs::create_dir_all(&dir).and_then(|_| fs::write(&path, &bytes)) {
            Ok(()) => {
                let _ = fs::remove_file(&global);
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Could not write snapshot to per-workspace location; using global path"
                );
            }
        }
        return Ok(Some(file));
    }

    Ok(None)
}

/// Read all workspace snapshots from `{data_dir}/workspaces/*/workflow_snapshot.json`.
/// Used at startup to load workflows from all workspaces into memory.
pub fn read_all_workspace_snapshots(
    data_dir: &Path,
) -> crate::error::Result<Vec<PersistedWorkflowRecord>> {
    let workspaces_dir = data_dir.join("workspaces");
    let mut all_records = Vec::new();

    if let Ok(entries) = fs::read_dir(&workspaces_dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let snap = entry.path().join(SNAPSHOT_FILENAME);
            if !snap.exists() {
                continue;
            }
            let bytes = match fs::read(&snap) {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!(path = %snap.display(), error = %e, "Failed to read workspace snapshot");
                    continue;
                }
            };
            let file: WorkflowSnapshotFile = match serde_json::from_slice(&bytes) {
                Ok(f) => f,
                Err(e) => {
                    tracing::warn!(path = %snap.display(), error = %e, "Failed to parse workspace snapshot");
                    continue;
                }
            };
            if file.version != SNAPSHOT_VERSION {
                tracing::warn!(path = %snap.display(), version = file.version, "Skipping snapshot with unsupported version");
                continue;
            }
            let ws_name = entry.file_name().to_string_lossy().into_owned();
            let mut records = file.workflows;
            // Backfill workspace_name for any records missing it.
            for rec in &mut records {
                if rec.workspace_name.is_empty() {
                    rec.workspace_name = ws_name.clone();
                }
            }
            all_records.extend(records);
        }
    }

    // Also check for a legacy global snapshot at `{data_dir}/workflow_snapshot.json`.
    let global = data_dir.join(SNAPSHOT_FILENAME);
    if global.exists() {
        let bytes = fs::read(&global)?;
        let file: WorkflowSnapshotFile =
            serde_json::from_slice(&bytes).map_err(|e| crate::config::ConfigError::Snapshot {
                op: "serde_json",
                detail: e.to_string(),
            })?;
        if file.version == SNAPSHOT_VERSION && !file.workflows.is_empty() {
            tracing::info!(
                count = file.workflows.len(),
                "Migrating workflows from legacy global snapshot"
            );
            all_records.extend(file.workflows);
        }
    }

    Ok(all_records)
}

/// Remove the legacy global snapshot file after migration.
/// Should be called by the persistence layer after successfully loading all workspace snapshots.
pub fn cleanup_legacy_global_snapshot(data_dir: &Path) {
    let global = data_dir.join(SNAPSHOT_FILENAME);
    if global.exists() {
        if let Err(e) = fs::remove_file(&global) {
            tracing::warn!(path = %global.display(), error = %e, "Failed to remove legacy global snapshot");
        } else {
            tracing::info!(path = %global.display(), "Removed legacy global snapshot after migration");
        }
    }
}

/// Write per-workspace snapshots by grouping records by `workspace_name`.
/// Each group is written to `{data_dir}/workspaces/{name}/workflow_snapshot.json`.
pub fn write_all_workspace_snapshots(
    data_dir: &Path,
    workflows: &[PersistedWorkflowRecord],
) -> crate::error::Result<()> {
    let mut by_workspace: HashMap<String, Vec<&PersistedWorkflowRecord>> = HashMap::new();
    for rec in workflows {
        by_workspace
            .entry(rec.workspace_name.clone())
            .or_default()
            .push(rec);
    }

    for (ws_name, records) in &by_workspace {
        if ws_name.is_empty() {
            continue; // skip records with no workspace (shouldn't happen)
        }
        let dir = data_dir.join("workspaces").join(ws_name);
        fs::create_dir_all(&dir)?;
        let path = dir.join(SNAPSHOT_FILENAME);
        let tmp = path.with_extension("json.tmp");
        let file = WorkflowSnapshotFile {
            version: SNAPSHOT_VERSION,
            workflows: records.iter().map(|r| (*r).clone()).collect(),
        };
        let json = serde_json::to_string_pretty(&file).map_err(|e| {
            crate::config::ConfigError::Snapshot {
                op: "serde_json",
                detail: e.to_string(),
            }
        })?;
        fs::write(&tmp, json)?;
        fs::rename(&tmp, &path)?;
    }

    // Clean up snapshot files for workspaces that no longer have any workflows.
    let workspaces_dir = data_dir.join("workspaces");
    if let Ok(entries) = fs::read_dir(&workspaces_dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let ws_name = entry.file_name().to_string_lossy().into_owned();
            if !by_workspace.contains_key(&ws_name) {
                let snap = entry.path().join(SNAPSHOT_FILENAME);
                if snap.exists() {
                    let _ = fs::remove_file(&snap);
                }
            }
        }
    }

    Ok(())
}

pub fn remove_workflow_snapshot(repo_path: &Path) -> crate::error::Result<()> {
    let path = snapshot_path(repo_path);
    if path.exists() {
        fs::remove_file(&path)?;
    }
    // Also clean up the legacy global snapshot if it exists.
    let global = resolve_snapshot_dir(repo_path).join(SNAPSHOT_FILENAME);
    if global.exists() && global != path {
        let _ = fs::remove_file(&global);
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
            workspace_name: "my-repo".into(),
            repository_id: Some("repo-uuid-1".into()),
            user_id: Some("user-1".into()),
            auth_pin: None,
        };
        let json = serde_json::to_string(&rec).unwrap();
        let back: PersistedWorkflowRecord = serde_json::from_str(&json).unwrap();
        assert!(!back.driver_started);
        assert_eq!(back.workspace_name, "my-repo");
        assert_eq!(back.user_id, Some("user-1".into()));
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

    #[test]
    fn workspace_name_from_normal_path() {
        assert_eq!(
            workspace_name_from_repo_path(Path::new("/workspaces/my-repo")),
            "my-repo"
        );
    }

    #[test]
    fn workspace_name_from_trailing_slash() {
        // Path::file_name returns None for paths ending in /
        // but PathBuf normalizes trailing slashes
        assert_eq!(
            workspace_name_from_repo_path(Path::new("/workspaces/my-repo/")),
            "my-repo"
        );
    }

    #[test]
    fn workspace_name_from_root_path() {
        assert_eq!(workspace_name_from_repo_path(Path::new("/")), "default");
    }

    #[test]
    fn workspace_name_from_single_component() {
        assert_eq!(
            workspace_name_from_repo_path(Path::new("workspace")),
            "workspace"
        );
    }

    // ── scan_in_flight_workflow_keys ─────────────────────────────────────

    fn rec(ticket: &str, state: WorkflowState) -> PersistedWorkflowRecord {
        PersistedWorkflowRecord {
            id: uuid::Uuid::new_v4().to_string(),
            ticket_key: ticket.into(),
            ticket_summary: String::new(),
            ticket_description: String::new(),
            ticket_type: "Task".into(),
            state,
            started_at: Utc::now(),
            updated_at: Utc::now(),
            steps_log: vec![],
            branch_name: String::new(),
            worktree_path: None,
            pr_url: None,
            pr_merged: false,
            terminal_lines: vec![],
            current_step_label: None,
            started_manually: false,
            jira_available: false,
            last_session_id: None,
            description_session_id: None,
            ticketing_system: crate::config::TicketingSystem::None,
            ticket_url: None,
            driver_started: false,
            workflow_def_runs: HashMap::new(),
            worktree_bootstrapped: false,
            workspace_name: "ws".into(),
            repository_id: None,
            user_id: None,
            auth_pin: None,
        }
    }

    fn write_snapshot(data_dir: &Path, ws: &str, records: Vec<PersistedWorkflowRecord>) {
        let dir = data_dir.join("workspaces").join(ws);
        fs::create_dir_all(&dir).unwrap();
        let file = WorkflowSnapshotFile {
            version: SNAPSHOT_VERSION,
            workflows: records,
        };
        fs::write(
            dir.join(SNAPSHOT_FILENAME),
            serde_json::to_vec_pretty(&file).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn scan_returns_empty_when_workspaces_dir_missing() {
        let d = tempfile::tempdir().unwrap();
        let got = scan_in_flight_workflow_keys(d.path()).unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn scan_skips_terminal_and_paused_workflows() {
        let d = tempfile::tempdir().unwrap();
        write_snapshot(
            d.path(),
            "ws-1",
            vec![
                rec("DONE-1", WorkflowState::Done),
                rec("STOP-1", WorkflowState::Stopped),
                rec(
                    "ERR-1",
                    WorkflowState::Error {
                        source_state: Box::new(WorkflowState::Pending),
                        message: "boom".into(),
                    },
                ),
                rec(
                    "PAUSE-1",
                    WorkflowState::Paused {
                        source_state: Box::new(WorkflowState::AddressingTicket { pass: 1 }),
                    },
                ),
            ],
        );

        let got = scan_in_flight_workflow_keys(d.path()).unwrap();
        assert!(
            got.is_empty(),
            "no in-flight workflows expected; got {got:?}"
        );
    }

    #[test]
    fn scan_returns_in_flight_keys() {
        let d = tempfile::tempdir().unwrap();
        write_snapshot(
            d.path(),
            "ws-1",
            vec![
                rec("RUN-1", WorkflowState::AddressingTicket { pass: 1 }),
                rec("PENDING-1", WorkflowState::Pending),
                rec("DONE-1", WorkflowState::Done),
            ],
        );
        write_snapshot(
            d.path(),
            "ws-2",
            vec![rec("REVIEW-1", WorkflowState::Reviewing)],
        );

        let mut got = scan_in_flight_workflow_keys(d.path()).unwrap();
        got.sort();
        assert_eq!(got, vec!["PENDING-1", "REVIEW-1", "RUN-1"]);
    }

    #[test]
    fn scan_tolerates_malformed_snapshot_files() {
        let d = tempfile::tempdir().unwrap();
        // One good workspace with an in-flight workflow.
        write_snapshot(
            d.path(),
            "ws-good",
            vec![rec("RUN-1", WorkflowState::AddressingTicket { pass: 1 })],
        );
        // One malformed snapshot — we expect the scanner to skip it.
        let bad_dir = d.path().join("workspaces").join("ws-bad");
        fs::create_dir_all(&bad_dir).unwrap();
        fs::write(bad_dir.join(SNAPSHOT_FILENAME), b"not valid json").unwrap();

        let got = scan_in_flight_workflow_keys(d.path()).unwrap();
        assert_eq!(got, vec!["RUN-1"]);
    }
}
