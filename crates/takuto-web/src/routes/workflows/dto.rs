// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Workflow response DTOs and the helpers that derive them from a `Workflow`.

use std::collections::HashMap;

use serde::Serialize;
use ts_rs::TS;

use takuto_core::container::ContainerRunner;
use takuto_core::jira::ticket_browse_url;
use takuto_core::workflow::engine::{TerminalLine, Workflow};
use takuto_core::workflow::state::WorkflowState;
use takuto_core::workflow::step::StepLog;

#[derive(Serialize, TS)]
#[ts(rename = "TerminalLine", export_to = "TerminalLine.ts")]
pub struct TerminalLineDto {
    pub text: String,
    pub stream: String,
}

impl From<&TerminalLine> for TerminalLineDto {
    fn from(tl: &TerminalLine) -> Self {
        Self {
            text: tl.text.clone(),
            stream: tl.stream.clone(),
        }
    }
}

#[derive(Serialize, TS)]
#[ts(export_to = "WorkflowSummary.ts")]
pub struct WorkflowSummary {
    pub id: String,
    pub ticket_key: String,
    pub ticket_summary: String,
    pub ticket_description: String,
    pub ticket_type: String,
    pub state: String,
    pub started_at: String,
    pub updated_at: String,
    pub branch_name: String,
    pub pr_url: Option<String>,
    pub pr_merged: bool,
    pub steps_log: Vec<StepLog>,
    pub error: Option<String>,
    pub terminal_lines: Vec<TerminalLineDto>,
    /// **Address PR Comments** is allowed (main flow **Done** and `pr_url` set).
    pub can_address_pr_comments: bool,
    /// **Merge base branch** is allowed (main flow **Done**, `pr_url` set, worktree exists).
    pub can_merge_base: bool,
    /// **Mark as Done** is allowed (workflow state is **Done**).
    pub can_mark_done: bool,
    /// **Delete** is allowed when the workflow is not **running** (`WorkflowState::is_active` is false),
    /// or when the workflow is on the dashboard but the driver has not been started yet.
    pub can_delete: bool,
    /// **Start** is allowed (workflow on dashboard but driver not yet spawned).
    pub can_start: bool,
    /// Step-based progress 0–100 (see `dashboard_progress` in takuto-core).
    pub progress_percent: u8,
    /// Estimated step count for the current phase (discrete progress segments / `N` in `k/N`).
    pub progress_steps_total: u32,
    /// Started via dashboard **+** manual picker.
    pub started_manually: bool,
    /// Counts against **`[general] max_concurrent_manual_workflows`** (manual start and not Done/Stopped/Error).
    pub counts_toward_manual_cap: bool,
    /// Jira **browse** URL from **`[jira] site`** + **`ticket_key`** (dashboard **Go to ticket**).
    pub jira_browse_url: String,
    /// Direct URL to the issue in the ticketing system. For GitHub workflows, this is the
    /// `html_url` stored when the workflow was created. For Jira workflows, it is the
    /// computed browse URL. `None` when no URL is available (e.g. `ticketing_system = none`).
    pub issue_url: Option<String>,
    /// **Open editor** is allowed: Docker is available and either the worktree
    /// exists or the workflow is terminal (Done/Stopped/Error) with a branch —
    /// in which case the worktree is recreated on demand when missing.
    pub can_open_editor: bool,
    /// Set when an editor container is already running for this workflow.
    pub editor_url: Option<String>,
    /// `(container_port, proxy_url)` pairs for user-configured application ports.
    pub editor_port_mappings: Vec<(u16, String)>,
    /// `true` when Jira (acli) was available when this workflow was created.
    pub jira_available: bool,
    /// Which ticketing system was active when this workflow was created: `"jira"`, `"github"`, or `"none"`.
    pub ticketing_system: String,
    /// **Resume from error** is allowed (Error or Stopped, worktree exists on disk).
    pub can_resume_from_error: bool,
    /// Set when a web terminal (ttyd) is running for this workflow's editor container.
    pub terminal_url: Option<String>,
    /// Configured run commands (from `[[run_commands]]` in config), with current running status.
    pub run_commands: Vec<RunCommandStatus>,
    /// Whether report generation is enabled in config (`[general] generate_report`).
    pub generate_report: bool,
    /// Whether a generated report file exists at `lore/reports/<key>_report.md` in the worktree.
    pub has_report: bool,
    /// Status of each dynamic workflow definition run for this ticket: def_name -> state display name.
    ///
    /// The wire field is `definition_runs` (the Rust field keeps the old
    /// name until a future engine refactor renames
    /// `Workflow.workflow_def_runs` → `WorkItem.definition_runs`).
    #[serde(rename = "definition_runs")]
    // ts-rs renders `HashMap<K, V>` with optional values (`{ [k]?: V }`); pin
    // it to `Record<string, string>` so consumers keep the prior shape.
    #[ts(rename = "definition_runs", type = "Record<string, string>")]
    pub workflow_def_runs: HashMap<String, String>,
    /// Absolute path of the git worktree on disk, if it exists.
    /// `None` while the worktree is still being pre-created in the background.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub worktree_path: Option<String>,
    /// ID of the user who created this workflow. `None` for legacy/poller workflows.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub user_id: Option<String>,
    /// Name of the repository (`repositories.name`) this workflow runs
    /// against. Powers the per-card repo badge on the dashboard. Always
    /// populated — every workflow has a `workspace_name`.
    pub workspace_name: String,
    /// FK to `repositories.id`. `None` for legacy snapshots not yet
    /// back-filled by the startup reconciliation.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub repository_id: Option<String>,
    /// Readiness of a **parked** item (added to the dashboard, not yet started):
    /// `"preparing"` (worktree pre-creation in flight), `"repo_not_ready"` (the
    /// repository isn't available on disk), or `"ready"` (start a workflow).
    /// `None` for any non-parked item (running / paused / terminal) — the card
    /// then shows the normal step/progress display. Backend-derived; the UI must
    /// not infer this from `branch_name`/`worktree_path`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub prep_state: Option<String>,
}

#[derive(Serialize, TS)]
#[ts(rename = "WorkflowCounts", export_to = "WorkflowCounts.ts")]
pub struct WorkflowCountsResponse {
    pub running: u32,
    pub completed: u32,
    pub errors: u32,
    pub paused: u32,
    pub pending: u32,
}

/// Status of a single run command.
#[derive(Serialize, TS)]
#[ts(export_to = "RunCommandStatus.ts")]
pub struct RunCommandStatus {
    /// Index of the command in the `[[run_commands]]` config array.
    pub index: usize,
    /// Display name from config.
    pub name: String,
    /// Whether the command is currently running.
    pub running: bool,
    /// Forwarded port `(container_port, proxy_url)`, if detected.
    pub forwarded_port: Option<(u16, String)>,
}

pub(super) fn workflow_action_flags(w: &Workflow) -> (bool, bool, bool) {
    let done = matches!(w.state, WorkflowState::Done);
    let has_pr = w
        .pr_url
        .as_deref()
        .map(str::trim)
        .is_some_and(|s| !s.is_empty());
    let has_worktree = w.worktree_path.as_ref().is_some_and(|p| p.exists());
    let can_address = done && has_pr;
    let can_merge_base = done && has_pr && has_worktree;
    let can_mark = done;
    (can_address, can_merge_base, can_mark)
}

pub(super) fn manual_cap_fields(w: &Workflow) -> (bool, bool) {
    let toward = w.started_manually && w.state.occupies_concurrency_slot();
    (w.started_manually, toward)
}

/// Docker-independent half of [`can_open_editor`]: whether the workflow has a
/// worktree on disk *or* one that can be recreated on demand.
///
/// A terminal (Done/Stopped/Error) workflow that still has a branch can have
/// its worktree recreated on demand (`ensure_worktree` in `editor.rs`), so the
/// editor stays reachable even after the worktree directory was pruned.
pub(super) fn editor_worktree_available(w: &Workflow) -> bool {
    let worktree_exists = w.worktree_path.as_ref().is_some_and(|p| p.exists());
    let recreatable = w.state.is_terminal() && !w.branch_name.is_empty();
    worktree_exists || recreatable
}

pub(super) fn can_open_editor(w: &Workflow) -> bool {
    // Docker is always required; the rest of the decision is pure and tested
    // via `editor_worktree_available`.
    ContainerRunner::is_available() && editor_worktree_available(w)
}

pub(super) fn has_report_file(w: &Workflow) -> bool {
    w.worktree_path.as_ref().is_some_and(|p| {
        p.join(format!("lore/reports/{}_report.md", w.ticket_key))
            .exists()
    })
}

pub(super) fn can_start_workflow(w: &Workflow) -> bool {
    matches!(w.state, WorkflowState::Pending) && !w.driver_started
}

/// Readiness signal for a **parked** item (see [`WorkflowSummary::prep_state`]).
/// `None` for any non-parked item. `repo_available` is whether the workflow's
/// repository is present on disk (resolved by the caller from the already-loaded
/// repo list). Pure — never inferred from branch/worktree existence, so it
/// cannot latch.
pub(super) fn prep_state(w: &Workflow, repo_available: bool) -> Option<&'static str> {
    // A running workflow in its bootstrap (setup) phase — before the flow's
    // first step — shows "Preparing worktree…" instead of a progress bar. These
    // states are only entered during bootstrap (assign / retrieve / create
    // worktree / mise / init); the first flow step transitions to
    // AddressingTicket, which clears this.
    if matches!(
        w.state,
        WorkflowState::Assigning
            | WorkflowState::RetrievingDetails
            | WorkflowState::CreatingWorktree
    ) {
        return Some("preparing");
    }
    if !can_start_workflow(w) {
        return None;
    }
    Some(if w.worktree_preparing {
        "preparing"
    } else if !repo_available {
        "repo_not_ready"
    } else {
        "ready"
    })
}

pub(super) fn can_resume_from_error(w: &Workflow) -> bool {
    matches!(
        w.state,
        WorkflowState::Error { .. } | WorkflowState::Stopped
    ) && w.worktree_path.as_ref().is_some_and(|p| p.exists())
}

/// Compute the canonical issue URL for a workflow.
///
/// - If `ticket_url` is `Some`, use it directly (GitHub issues and future providers).
/// - If `ticket_url` is `None` and the ticketing system is Jira with a configured site,
///   compute the Jira browse URL.
/// - Otherwise `None`.
pub(super) fn build_issue_url(w: &Workflow, jira_site: &str) -> Option<String> {
    if let Some(ref url) = w.ticket_url {
        return Some(url.clone());
    }
    if w.ticketing_system == takuto_core::config::TicketingSystem::Jira && !jira_site.is_empty() {
        let url = ticket_browse_url(jira_site, &w.ticket_key);
        if !url.is_empty() {
            return Some(url);
        }
    }
    None
}

pub(super) fn workflow_def_runs_display(w: &Workflow) -> HashMap<String, String> {
    w.workflow_def_runs
        .iter()
        .map(|(k, v)| (k.clone(), v.display_name().to_string()))
        .collect()
}

pub(super) fn extract_error(state: &WorkflowState) -> Option<String> {
    match state {
        WorkflowState::Error { message, .. } => Some(message.clone()),
        _ => None,
    }
}

/// Build the run command status list for a given workflow's ticket key.
pub(super) fn build_run_commands_status(
    configured: &[takuto_core::db::user_worktree_commands::RunCommand],
    active_cmds: Option<&Vec<crate::state::ActiveRunCommand>>,
) -> Vec<RunCommandStatus> {
    configured
        .iter()
        .enumerate()
        .map(|(i, rc)| {
            let (running, forwarded_port) = if let Some(active) = active_cmds {
                if let Some(cmd_state) = active.iter().find(|c| c.cmd_index == i) {
                    (
                        true,
                        cmd_state
                            .forwarded_port
                            .as_ref()
                            .map(|f| (f.container_port, f.proxy_url.clone())),
                    )
                } else {
                    (false, None)
                }
            } else {
                (false, None)
            };
            RunCommandStatus {
                index: i,
                name: rc.name.clone(),
                running,
                forwarded_port,
            }
        })
        .collect()
}

#[cfg(test)]
mod ts_bindings {
    use super::*;
    use ts_rs::TS;

    /// Regenerate the committed `ui/src/api/generated/*.ts` mirror of these
    /// DTOs. `export_all_to` also emits each transitive dependency
    /// (`StepLog`, `StepStatus`, …) into the same directory. CI re-runs this
    /// and `git diff --exit-code`s the directory, so a Rust DTO change that
    /// isn't reflected in committed TS fails the build. See
    /// `crates/takuto-web/src/ts_bindings.rs` for the shared output path.
    #[test]
    fn export_workflow_dtos() {
        let out = crate::ts_bindings::generated_dir();
        std::fs::create_dir_all(&out).expect("create generated dir");
        WorkflowSummary::export_all_to(&out).expect("export WorkflowSummary");
        WorkflowCountsResponse::export_all_to(&out).expect("export WorkflowCounts");
        RunCommandStatus::export_all_to(&out).expect("export RunCommandStatus");
        TerminalLineDto::export_all_to(&out).expect("export TerminalLine");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal `Workflow` in `Pending` state with the given `driver_started` value.
    fn wf_pending(driver_started: bool) -> Workflow {
        let mut w = Workflow::new(
            "T-1".into(),
            "summary".into(),
            true,
            false,
            takuto_core::config::TicketingSystem::None,
            None,
            "test-workspace".into(),
        );
        w.driver_started = driver_started;
        w
    }

    #[test]
    fn can_start_pending_not_started() {
        assert!(can_start_workflow(&wf_pending(false)));
    }

    #[test]
    fn can_start_false_when_started() {
        assert!(!can_start_workflow(&wf_pending(true)));
    }

    #[test]
    fn can_start_false_when_not_pending() {
        let mut w = wf_pending(false);
        w.state = WorkflowState::Done;
        assert!(!can_start_workflow(&w));
    }

    // ── prep_state (parked-item readiness) ────────────────────────────────

    #[test]
    fn prep_state_ready_for_parked_with_repo() {
        let w = wf_pending(false); // Pending, no driver, not preparing
        assert_eq!(prep_state(&w, true), Some("ready"));
    }

    #[test]
    fn prep_state_preparing_when_worktree_in_flight() {
        let mut w = wf_pending(false);
        w.worktree_preparing = true;
        // "preparing" wins even if the repo isn't (yet) resolvable.
        assert_eq!(prep_state(&w, false), Some("preparing"));
    }

    #[test]
    fn prep_state_repo_not_ready_when_repo_absent() {
        let w = wf_pending(false);
        assert_eq!(prep_state(&w, false), Some("repo_not_ready"));
    }

    #[test]
    fn prep_state_none_for_non_parked() {
        // Driver started → not parked.
        let started = wf_pending(true);
        assert_eq!(prep_state(&started, true), None);
        // Terminal → not parked.
        let mut done = wf_pending(false);
        done.state = WorkflowState::Done;
        assert_eq!(prep_state(&done, true), None);
    }

    #[test]
    fn prep_state_preparing_during_bootstrap_states() {
        // A running workflow in its bootstrap phase shows "Preparing worktree…"
        // (the bar is hidden until the flow's first step).
        for s in [
            WorkflowState::Assigning,
            WorkflowState::RetrievingDetails,
            WorkflowState::CreatingWorktree,
        ] {
            let mut w = wf_pending(true);
            w.state = s.clone();
            assert_eq!(prep_state(&w, true), Some("preparing"), "{s:?}");
        }
        // Once the flow's first step runs, it's no longer "preparing".
        let mut flow = wf_pending(true);
        flow.state = WorkflowState::AddressingTicket { pass: 1 };
        assert_eq!(prep_state(&flow, true), None);
    }

    /// Create a unique existing directory under the system temp dir to stand in
    /// for a live worktree path.
    fn make_existing_dir(tag: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "takuto-editor-test-{}-{tag}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create temp worktree dir");
        dir
    }

    /// Terminal workflow (Done) that still carries a branch is recreatable even
    /// when the worktree directory is gone — the editor decision is `true`
    /// (Docker gate applied separately in `can_open_editor`).
    #[test]
    fn editor_available_terminal_with_branch_no_worktree() {
        let mut w = wf_pending(true);
        w.state = WorkflowState::Done;
        w.branch_name = "feat/gh-7".into();
        w.worktree_path = None;
        assert!(editor_worktree_available(&w));
    }

    #[test]
    fn editor_available_terminal_stopped_with_branch() {
        let mut w = wf_pending(true);
        w.state = WorkflowState::Stopped;
        w.branch_name = "feat/gh-3".into();
        w.worktree_path = None;
        assert!(editor_worktree_available(&w));
    }

    #[test]
    fn editor_available_terminal_error_with_branch() {
        let mut w = wf_pending(true);
        w.state = WorkflowState::Error {
            source_state: Box::new(WorkflowState::Pending),
            message: "boom".into(),
        };
        w.branch_name = "feat/gh-9".into();
        w.worktree_path = None;
        assert!(editor_worktree_available(&w));
    }

    /// Pending workflow is not recreatable and (in tests) has no worktree → false.
    #[test]
    fn editor_unavailable_pending_no_worktree() {
        let mut w = wf_pending(false);
        w.branch_name = "feat/whatever".into();
        w.worktree_path = None;
        assert!(!editor_worktree_available(&w));
    }

    /// Terminal but no branch to check out → not recreatable → false.
    #[test]
    fn editor_unavailable_terminal_no_branch() {
        let mut w = wf_pending(true);
        w.state = WorkflowState::Done;
        w.branch_name = String::new();
        w.worktree_path = None;
        assert!(!editor_worktree_available(&w));
    }

    /// A worktree that exists on disk makes the editor available regardless of
    /// state (even a non-terminal/Pending workflow).
    #[test]
    fn editor_available_when_worktree_exists() {
        let dir = make_existing_dir("exists");
        let mut w = wf_pending(false);
        w.state = WorkflowState::Pending;
        w.branch_name = String::new();
        w.worktree_path = Some(dir.clone());
        assert!(editor_worktree_available(&w));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A stale `worktree_path` that no longer exists on disk does not count as a
    /// live worktree; only the recreatable/terminal path can rescue it.
    #[test]
    fn editor_unavailable_when_worktree_path_missing_and_pending() {
        let dir = make_existing_dir("missing");
        std::fs::remove_dir_all(&dir).expect("remove dir to simulate pruned worktree");
        let mut w = wf_pending(false);
        w.state = WorkflowState::Pending;
        w.branch_name = "feat/pruned".into();
        w.worktree_path = Some(dir);
        assert!(!editor_worktree_available(&w));
    }

    /// `can_open_editor` ANDs the worktree decision with Docker availability.
    /// When the worktree decision is `false` (Pending/no-branch/no-worktree),
    /// the editor must be unavailable regardless of whether Docker is present.
    #[test]
    fn can_open_editor_false_when_worktree_decision_false() {
        let mut w = wf_pending(false);
        w.state = WorkflowState::Pending;
        w.branch_name = String::new();
        w.worktree_path = None;
        assert!(!editor_worktree_available(&w));
        assert!(!can_open_editor(&w));
    }
}
