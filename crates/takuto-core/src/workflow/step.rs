// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export_to = "StepLog.ts")]
pub struct StepLog {
    pub step_name: String,
    /// RFC3339 timestamp on the wire (serde serializes `DateTime<Utc>` as a string).
    #[ts(type = "string")]
    pub started_at: DateTime<Utc>,
    #[ts(type = "string | null")]
    pub completed_at: Option<DateTime<Utc>>,
    pub status: StepStatus,
    pub output: Vec<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export_to = "StepStatus.ts")]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_status_serde_round_trip() {
        for status in [
            StepStatus::Running,
            StepStatus::Success,
            StepStatus::Failed,
            StepStatus::Skipped,
        ] {
            let json = serde_json::to_string(&status).unwrap();
            let back: StepStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(back, status);
        }
    }

    #[test]
    fn step_log_new_has_correct_initial_fields() {
        let log = StepLog::new("Build project".to_string());
        assert_eq!(log.step_name, "Build project");
        assert_eq!(log.status, StepStatus::Running);
        assert!(log.completed_at.is_none());
        assert!(log.output.is_empty());
        assert!(log.error.is_none());
        // started_at should be very recent
        let elapsed = Utc::now() - log.started_at;
        assert!(elapsed.num_seconds() < 2);
    }

    #[test]
    fn step_log_complete_sets_status_and_timestamp() {
        let mut log = StepLog::new("Test step".to_string());
        assert!(log.completed_at.is_none());

        log.complete(StepStatus::Success);
        assert_eq!(log.status, StepStatus::Success);
        assert!(log.completed_at.is_some());
        assert!(log.error.is_none());
    }

    #[test]
    fn step_log_fail_sets_error_and_failed_status() {
        let mut log = StepLog::new("Failing step".to_string());

        log.fail("timeout exceeded".to_string());
        assert_eq!(log.status, StepStatus::Failed);
        assert!(log.completed_at.is_some());
        assert_eq!(log.error.as_deref(), Some("timeout exceeded"));
    }

    #[test]
    fn step_log_transition_running_to_success() {
        let mut log = StepLog::new("Step".to_string());
        assert_eq!(log.status, StepStatus::Running);
        log.complete(StepStatus::Success);
        assert_eq!(log.status, StepStatus::Success);
    }

    #[test]
    fn step_log_transition_running_to_failed() {
        let mut log = StepLog::new("Step".to_string());
        assert_eq!(log.status, StepStatus::Running);
        log.fail("crash".to_string());
        assert_eq!(log.status, StepStatus::Failed);
    }

    #[test]
    fn step_log_serde_round_trip() {
        let mut log = StepLog::new("Serialize me".to_string());
        log.output.push("line 1".to_string());
        log.complete(StepStatus::Success);

        let json = serde_json::to_string(&log).unwrap();
        let back: StepLog = serde_json::from_str(&json).unwrap();
        assert_eq!(back.step_name, "Serialize me");
        assert_eq!(back.status, StepStatus::Success);
        assert_eq!(back.output, vec!["line 1"]);
        assert!(back.completed_at.is_some());
    }
}
