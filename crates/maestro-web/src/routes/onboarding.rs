// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Phase 0 onboarding endpoint. Exposes the structured `SystemStatus`
//! snapshot the dashboard renders the degraded-mode banner from.
//!
//! Source-of-truth contract: `tmp/multi-agents/04_architecture.md §1.3`.
//! The endpoint is **public** (no auth) so the dashboard can poll it before a
//! user has signed in — matching `/api/auth/status`.

use axum::Json;
use axum::extract::State;

use maestro_core::docker_hooks::SystemStatus;

use crate::state::AppState;

/// `GET /api/onboarding/status` — returns the current `SystemStatus` snapshot.
///
/// Public endpoint (no auth required). The snapshot is captured at startup
/// and refreshed in place by `PUT /api/config/agent` (Phase 1), so callers
/// always see the latest provider / degraded state without a process restart.
pub async fn onboarding_status(State(state): State<AppState>) -> Json<SystemStatus> {
    let snapshot = state.system_status.read().await.clone();
    Json(snapshot)
}
