// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Per-candidate annotations for the manual-add ticket picker: whether an item
//! is already on the caller's board, and any PR a previous run recorded.

use std::collections::HashMap;
use std::sync::Arc;

use takuto_core::db::Database;
use takuto_core::workflow::engine::Workflow;
use takuto_core::workflow::state::WorkflowState;
use tokio::sync::RwLock;

/// Picker annotation for a single candidate ticket.
#[derive(Debug, Default, Clone)]
pub(crate) struct CandidateAnnotation {
    /// The caller already has this ticket on their board in a non-`Done` state
    /// (parked / in-progress / paused / stopped / errored) — a re-add must be
    /// blocked. `Done` items are treated as past work and stay re-addable.
    pub already_added: bool,
    /// The most recent PR a prior run recorded for this ticket, if any. Drives
    /// the "this will create a NEW PR" confirmation.
    pub existing_pr_url: Option<String>,
}

/// True when a board state means the item is currently "on the board" rather
/// than handled-in-the-past. Everything except `Done` counts.
fn occupies_board(state: &WorkflowState) -> bool {
    !matches!(state, WorkflowState::Done)
}

/// Annotate `keys` for the manual-add picker.
///
/// `already_added` comes from the engine's in-memory map (the live board),
/// scoped to `user_id` and — when `workspace_name` is provided — that workspace.
/// `existing_pr_url` prefers an in-memory PR (covers a just-completed `Done`
/// run still resident) and falls back to the DB history (covers PRs from
/// soft-deleted past runs after a restart) when a `workspace_name` is known.
pub(crate) async fn annotate_candidates(
    workflows: &Arc<RwLock<HashMap<String, Workflow>>>,
    db: Option<&Database>,
    user_id: &str,
    workspace_name: Option<&str>,
    keys: &[String],
) -> HashMap<String, CandidateAnnotation> {
    let mut out: HashMap<String, CandidateAnnotation> = keys
        .iter()
        .map(|k| (k.clone(), CandidateAnnotation::default()))
        .collect();

    // Pass 1 — the live in-memory board.
    {
        let map = workflows.read().await;
        for w in map.values() {
            if w.user_id.as_deref() != Some(user_id) {
                continue;
            }
            if let Some(ws) = workspace_name
                && w.workspace_name != ws
            {
                continue;
            }
            let Some(ann) = out.get_mut(&w.ticket_key) else {
                continue;
            };
            if occupies_board(&w.state) {
                ann.already_added = true;
            }
            if ann.existing_pr_url.is_none()
                && let Some(url) = w.pr_url.as_ref()
            {
                ann.existing_pr_url = Some(url.clone());
            }
        }
    }

    // Pass 2 — DB history for PRs from soft-deleted past runs, only when we know
    // the workspace and didn't already find a live PR.
    if let (Some(db), Some(ws)) = (db, workspace_name) {
        let adapter = db.adapter();
        for (key, ann) in out.iter_mut() {
            if ann.existing_pr_url.is_some() {
                continue;
            }
            if let Ok(Some(url)) =
                takuto_core::db::work_items::latest_pr_url_for_ticket(adapter, ws, key).await
            {
                ann.existing_pr_url = Some(url);
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use takuto_core::config::TicketingSystem;

    fn wf(key: &str, owner: &str, state: WorkflowState, pr: Option<&str>) -> Workflow {
        let mut w = Workflow::new(
            key.to_string(),
            "summary".to_string(),
            true,
            false,
            TicketingSystem::GitHub,
            None,
            "repo".to_string(),
        );
        w.user_id = Some(owner.to_string());
        w.state = state;
        w.pr_url = pr.map(str::to_string);
        w
    }

    async fn annotate(
        map: HashMap<String, Workflow>,
        keys: &[&str],
    ) -> HashMap<String, CandidateAnnotation> {
        let arc = Arc::new(RwLock::new(map));
        let keys: Vec<String> = keys.iter().map(|s| s.to_string()).collect();
        annotate_candidates(&arc, None, "u-alice", Some("repo"), &keys).await
    }

    #[tokio::test]
    async fn non_done_board_states_are_already_added() {
        let mut map = HashMap::new();
        map.insert(
            "GH-1".into(),
            wf(
                "GH-1",
                "u-alice",
                WorkflowState::AddressingTicket { pass: 1 },
                None,
            ),
        );
        map.insert(
            "GH-2".into(),
            wf("GH-2", "u-alice", WorkflowState::Stopped, None),
        );
        map.insert(
            "GH-3".into(),
            wf(
                "GH-3",
                "u-alice",
                WorkflowState::Paused {
                    source_state: Box::new(WorkflowState::AddressingTicket { pass: 1 }),
                },
                None,
            ),
        );
        map.insert(
            "GH-5".into(),
            wf(
                "GH-5",
                "u-alice",
                WorkflowState::Error {
                    source_state: Box::new(WorkflowState::Reviewing),
                    message: "x".into(),
                },
                None,
            ),
        );
        let ann = annotate(map, &["GH-1", "GH-2", "GH-3", "GH-5"]).await;
        for k in ["GH-1", "GH-2", "GH-3", "GH-5"] {
            assert!(
                ann[k].already_added,
                "{k} (live, non-Done) must be already_added"
            );
        }
    }

    #[tokio::test]
    async fn done_is_not_already_added_but_exposes_prior_pr() {
        let mut map = HashMap::new();
        map.insert(
            "GH-4".into(),
            wf(
                "GH-4",
                "u-alice",
                WorkflowState::Done,
                Some("https://github.com/o/r/pull/18"),
            ),
        );
        let ann = annotate(map, &["GH-4"]).await;
        assert!(
            !ann["GH-4"].already_added,
            "a Done item is past work, re-addable"
        );
        assert_eq!(
            ann["GH-4"].existing_pr_url.as_deref(),
            Some("https://github.com/o/r/pull/18"),
            "a Done item's recorded PR drives the re-add confirmation"
        );
    }

    #[tokio::test]
    async fn another_users_board_and_unknown_keys_are_clean() {
        let mut map = HashMap::new();
        map.insert(
            "GH-7".into(),
            wf("GH-7", "u-bob", WorkflowState::Stopped, None),
        );
        let ann = annotate(map, &["GH-7", "GH-99"]).await;
        // GH-7 belongs to bob, GH-99 is on nobody's board.
        assert!(!ann["GH-7"].already_added);
        assert!(!ann["GH-99"].already_added);
        assert!(ann["GH-99"].existing_pr_url.is_none());
    }
}
