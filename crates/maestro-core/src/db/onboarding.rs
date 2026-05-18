// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! `onboarding_state` table — row shape only.
//!
//! Phase 2a foundation: each admin gets one row tracking which of the four
//! wizard steps were completed vs skipped. Phase 2b wires this to the
//! `POST /api/onboarding/{skip,complete}` endpoints.

use serde::{Deserialize, Serialize};

/// Tri-state for a single wizard step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnboardingStepState {
    Completed,
    Skipped,
}

impl OnboardingStepState {
    pub fn as_str(self) -> &'static str {
        match self {
            OnboardingStepState::Completed => "completed",
            OnboardingStepState::Skipped => "skipped",
        }
    }
}

/// One row of onboarding wizard state. `None` for any step that has not been
/// reached yet (the wizard renders the unreached step normally).
#[derive(Debug, Clone)]
pub struct OnboardingStateRow {
    pub user_id: String,
    pub step_1_ticketing: Option<OnboardingStepState>,
    pub step_2_provider: Option<OnboardingStepState>,
    pub step_3_github: Option<OnboardingStepState>,
    pub step_4_credentials: Option<OnboardingStepState>,
    /// ISO-8601 UTC timestamp; `Some(_)` once the wizard is dismissed.
    /// Clearing this re-enters the wizard (FR-2.4).
    pub completed_at: Option<String>,
    pub updated_at: String,
}
