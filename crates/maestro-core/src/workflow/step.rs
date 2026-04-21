// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepLog {
    pub step_name: String,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub status: StepStatus,
    pub output: Vec<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StepStatus {
    Running,
    Success,
    Failed,
    Skipped,
}

impl StepLog {
    pub fn new(step_name: String) -> Self {
        Self {
            step_name,
            started_at: Utc::now(),
            completed_at: None,
            status: StepStatus::Running,
            output: Vec::new(),
            error: None,
        }
    }

    pub fn complete(&mut self, status: StepStatus) {
        self.completed_at = Some(Utc::now());
        self.status = status;
    }

    pub fn fail(&mut self, error: String) {
        self.completed_at = Some(Utc::now());
        self.status = StepStatus::Failed;
        self.error = Some(error);
    }
}
