use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkflowState {
    Pending,
    Assigning,
    RetrievingDetails,
    CreatingWorktree,
    AddressingTicket { pass: u8 },
    /// Secondary PR-comment workflow after main flow reached Done (uses `[[review_agent_steps]]`).
    AddressingPrComments { pass: u8 },
    Reviewing,
    CreatingPR,
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
}

impl std::fmt::Display for WorkflowState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display_name())
    }
}
