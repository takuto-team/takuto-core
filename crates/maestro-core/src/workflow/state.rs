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
