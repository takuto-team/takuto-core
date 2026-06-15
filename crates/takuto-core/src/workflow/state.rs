// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkflowState {
    Pending,
    Assigning,
    RetrievingDetails,
    CreatingWorktree,
    AddressingTicket {
        pass: u8,
    },
    /// Legacy state — retained for `workflow_snapshot.json` backward compatibility only.
    /// Not driven by the engine.
    AddressingPrComments {
        pass: u8,
    },
    /// Legacy state — retained for `workflow_snapshot.json` backward compatibility only.
    /// Not driven by the engine.
    MergingBaseBranch {
        pass: u8,
    },
    Reviewing,
    CreatingPR,
    /// Manual `workflow_snapshot.json` edits often use lowercase — accept both.
    #[serde(alias = "done")]
    Done,
    Error {
        source_state: Box<WorkflowState>,
        message: String,
    },
    Paused {
        source_state: Box<WorkflowState>,
    },
    Stopped,
}

impl WorkflowState {
    pub fn display_name(&self) -> String {
        match self {
            Self::Pending => "Pending".to_string(),
            Self::Assigning => "Assigning Ticket".to_string(),
            Self::RetrievingDetails => "Retrieving Details".to_string(),
            Self::CreatingWorktree => "Creating Worktree".to_string(),
            Self::AddressingTicket { .. } => "Running agent steps".to_string(),
            Self::AddressingPrComments { .. } => "Addressing PR comments".to_string(),
            Self::MergingBaseBranch { .. } => "Merging base branch".to_string(),
            Self::Reviewing => "Reviewing Changes".to_string(),
            Self::CreatingPR => "Creating PR".to_string(),
            Self::Done => "Done".to_string(),
            Self::Error { message, .. } => format!("Error: {message}"),
            Self::Paused { .. } => "Paused".to_string(),
            Self::Stopped => "Stopped".to_string(),
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Done | Self::Stopped | Self::Error { .. })
    }

    pub fn is_active(&self) -> bool {
        !self.is_terminal() && !matches!(self, Self::Paused { .. } | Self::Error { .. })
    }

    /// Tickets that are not **Done**, **Stopped**, or **Error** reserve capacity against
    /// `max_concurrent_workflows` for Jira polling (paused workflows count too).
    pub fn occupies_concurrency_slot(&self) -> bool {
        !matches!(self, Self::Done | Self::Stopped | Self::Error { .. })
    }
}

impl std::fmt::Display for WorkflowState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

#[cfg(test)]
mod tests {
    use super::WorkflowState;

    #[test]
    fn done_deserializes_lowercase_alias() {
        let s: WorkflowState = serde_json::from_str(r#""done""#).unwrap();
        assert!(matches!(s, WorkflowState::Done));
    }

    #[test]
    fn addressing_ticket_deserializes_externally_tagged() {
        let s: WorkflowState = serde_json::from_str(r#"{"AddressingTicket":{"pass":1}}"#).unwrap();
        assert!(matches!(s, WorkflowState::AddressingTicket { pass: 1 }));
    }
}

#[cfg(test)]
mod facade_state_tests {
    use super::WorkflowState;

    // -----------------------------------------------------------------------
    // 1. WorkflowState::display_name() — every variant
    // -----------------------------------------------------------------------

    #[test]
    fn display_name_pending() {
        assert_eq!(WorkflowState::Pending.display_name(), "Pending");
    }

    #[test]
    fn display_name_assigning() {
        assert_eq!(WorkflowState::Assigning.display_name(), "Assigning Ticket");
    }

    #[test]
    fn display_name_retrieving_details() {
        assert_eq!(
            WorkflowState::RetrievingDetails.display_name(),
            "Retrieving Details"
        );
    }

    #[test]
    fn display_name_creating_worktree() {
        assert_eq!(
            WorkflowState::CreatingWorktree.display_name(),
            "Creating Worktree"
        );
    }

    #[test]
    fn display_name_addressing_ticket() {
        assert_eq!(
            WorkflowState::AddressingTicket { pass: 1 }.display_name(),
            "Running agent steps"
        );
    }

    #[test]
    fn display_name_addressing_pr_comments() {
        assert_eq!(
            WorkflowState::AddressingPrComments { pass: 1 }.display_name(),
            "Addressing PR comments"
        );
    }

    #[test]
    fn display_name_merging_base_branch() {
        assert_eq!(
            WorkflowState::MergingBaseBranch { pass: 1 }.display_name(),
            "Merging base branch"
        );
    }

    #[test]
    fn display_name_reviewing() {
        assert_eq!(WorkflowState::Reviewing.display_name(), "Reviewing Changes");
    }

    #[test]
    fn display_name_creating_pr() {
        assert_eq!(WorkflowState::CreatingPR.display_name(), "Creating PR");
    }

    #[test]
    fn display_name_done() {
        assert_eq!(WorkflowState::Done.display_name(), "Done");
    }

    #[test]
    fn display_name_error() {
        let state = WorkflowState::Error {
            source_state: Box::new(WorkflowState::Assigning),
            message: "timeout".into(),
        };
        assert_eq!(state.display_name(), "Error: timeout");
    }

    #[test]
    fn display_name_paused() {
        let state = WorkflowState::Paused {
            source_state: Box::new(WorkflowState::AddressingTicket { pass: 1 }),
        };
        assert_eq!(state.display_name(), "Paused");
    }

    #[test]
    fn display_name_stopped() {
        assert_eq!(WorkflowState::Stopped.display_name(), "Stopped");
    }

    // -----------------------------------------------------------------------
    // 2. WorkflowState::is_terminal() — Done/Stopped/Error return true; others false
    // -----------------------------------------------------------------------

    #[test]
    fn is_terminal_done() {
        assert!(WorkflowState::Done.is_terminal());
    }

    #[test]
    fn is_terminal_stopped() {
        assert!(WorkflowState::Stopped.is_terminal());
    }

    #[test]
    fn is_terminal_error() {
        let state = WorkflowState::Error {
            source_state: Box::new(WorkflowState::Pending),
            message: "fail".into(),
        };
        assert!(state.is_terminal());
    }

    #[test]
    fn is_terminal_false_for_pending() {
        assert!(!WorkflowState::Pending.is_terminal());
    }

    #[test]
    fn is_terminal_false_for_assigning() {
        assert!(!WorkflowState::Assigning.is_terminal());
    }

    #[test]
    fn is_terminal_false_for_addressing_ticket() {
        assert!(!WorkflowState::AddressingTicket { pass: 1 }.is_terminal());
    }

    #[test]
    fn is_terminal_false_for_paused() {
        let state = WorkflowState::Paused {
            source_state: Box::new(WorkflowState::Assigning),
        };
        assert!(!state.is_terminal());
    }

    // -----------------------------------------------------------------------
    // 3. WorkflowState::is_active() — Paused and Error are not active
    // -----------------------------------------------------------------------

    #[test]
    fn is_active_true_for_pending() {
        assert!(WorkflowState::Pending.is_active());
    }

    #[test]
    fn is_active_true_for_assigning() {
        assert!(WorkflowState::Assigning.is_active());
    }

    #[test]
    fn is_active_true_for_addressing_ticket() {
        assert!(WorkflowState::AddressingTicket { pass: 1 }.is_active());
    }

    #[test]
    fn is_active_true_for_creating_worktree() {
        assert!(WorkflowState::CreatingWorktree.is_active());
    }

    #[test]
    fn is_active_false_for_done() {
        assert!(!WorkflowState::Done.is_active());
    }

    #[test]
    fn is_active_false_for_stopped() {
        assert!(!WorkflowState::Stopped.is_active());
    }

    #[test]
    fn is_active_false_for_error() {
        let state = WorkflowState::Error {
            source_state: Box::new(WorkflowState::Pending),
            message: "fail".into(),
        };
        assert!(!state.is_active());
    }

    #[test]
    fn is_active_false_for_paused() {
        let state = WorkflowState::Paused {
            source_state: Box::new(WorkflowState::Assigning),
        };
        assert!(!state.is_active());
    }

    // -----------------------------------------------------------------------
    // 4. WorkflowState::occupies_concurrency_slot() — Done/Stopped/Error do not
    // -----------------------------------------------------------------------

    #[test]
    fn occupies_slot_true_for_pending() {
        assert!(WorkflowState::Pending.occupies_concurrency_slot());
    }

    #[test]
    fn occupies_slot_true_for_assigning() {
        assert!(WorkflowState::Assigning.occupies_concurrency_slot());
    }

    #[test]
    fn occupies_slot_true_for_paused() {
        let state = WorkflowState::Paused {
            source_state: Box::new(WorkflowState::Assigning),
        };
        assert!(state.occupies_concurrency_slot());
    }

    #[test]
    fn occupies_slot_true_for_addressing_ticket() {
        assert!(WorkflowState::AddressingTicket { pass: 1 }.occupies_concurrency_slot());
    }

    #[test]
    fn occupies_slot_false_for_done() {
        assert!(!WorkflowState::Done.occupies_concurrency_slot());
    }

    #[test]
    fn occupies_slot_false_for_stopped() {
        assert!(!WorkflowState::Stopped.occupies_concurrency_slot());
    }

    #[test]
    fn occupies_slot_false_for_error() {
        let state = WorkflowState::Error {
            source_state: Box::new(WorkflowState::Pending),
            message: "fail".into(),
        };
        assert!(!state.occupies_concurrency_slot());
    }
}
