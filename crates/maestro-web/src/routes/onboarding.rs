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
use axum::http::HeaderMap;
use serde::Serialize;

use maestro_core::docker_hooks::SystemStatus;

use crate::state::AppState;

/// Wire shape of `GET /api/onboarding/status`. Wraps the boot-time
/// [`SystemStatus`] with optional per-user wizard state — when the caller
/// presents a valid session cookie, the response includes a `user_onboarding`
/// object reporting which of the four steps are completed / skipped (and
/// flips `step_4_credentials` to "completed" when the user has an active
/// provider credential row, even if they haven't clicked through the wizard
/// step explicitly).
#[derive(Debug, Serialize)]
pub struct OnboardingStatusBody {
    #[serde(flatten)]
    pub status: SystemStatus,
    /// `None` for unauthenticated callers; `Some` for users with a valid
    /// session even when they have no row yet (empty defaults).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_onboarding: Option<UserOnboardingSummary>,
}

#[derive(Debug, Serialize, Default)]
pub struct UserOnboardingSummary {
    pub step_1_ticketing: Option<String>,
    pub step_2_provider: Option<String>,
    pub step_3_github: Option<String>,
    pub step_4_credentials: Option<String>,
    pub completed_at: Option<String>,
}

/// `GET /api/onboarding/status` — returns the current `SystemStatus` snapshot.
///
/// Public endpoint (no auth required). The snapshot is captured at startup
/// and refreshed in place by `PUT /api/config/agent` (Phase 1), so callers
/// always see the latest provider / degraded state without a process restart.
///
/// Phase 2b.1: when a session cookie is present and resolves to a user, the
/// response additionally includes that user's `onboarding_state` row (or
/// empty defaults), with `step_4_credentials` auto-flipping to "completed"
/// if the user has at least one active provider credential row — saving the
/// user a wizard click after they've already pasted their key.
pub async fn onboarding_status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Json<OnboardingStatusBody> {
    let status = state.system_status.read().await.clone();

    let active_provider = status.provider.selected.clone();
    let user_onboarding = if let Some(db) = state.db.as_ref() {
        let cookie = crate::auth::session_cookie_from_headers(&headers)
            .map(|s| s.to_string())
            .unwrap_or_default();
        if cookie.is_empty() {
            None
        } else {
            let db = db.clone();
            tokio::task::spawn_blocking(move || -> Option<UserOnboardingSummary> {
                let conn = db.conn().blocking_lock();
                let user_id = crate::auth::validate_db_session(&conn, &cookie)?;
                let row =
                    maestro_core::db::onboarding::get(&conn, &user_id).ok().flatten();
                let creds_present = matches!(
                    maestro_core::db::provider_credentials::find_active(
                        &conn,
                        &user_id,
                        &active_provider,
                    ),
                    Ok(Some(_))
                );
                let mut summary = match row {
                    Some(r) => UserOnboardingSummary {
                        step_1_ticketing: r.step_1_ticketing.map(|s| s.as_str().to_string()),
                        step_2_provider: r.step_2_provider.map(|s| s.as_str().to_string()),
                        step_3_github: r.step_3_github.map(|s| s.as_str().to_string()),
                        step_4_credentials: r.step_4_credentials.map(|s| s.as_str().to_string()),
                        completed_at: r.completed_at,
                    },
                    None => UserOnboardingSummary::default(),
                };
                if creds_present && summary.step_4_credentials.is_none() {
                    summary.step_4_credentials = Some("completed".to_string());
                }
                Some(summary)
            })
            .await
            .ok()
            .flatten()
        }
    } else {
        None
    };

    Json(OnboardingStatusBody {
        status,
        user_onboarding,
    })
}
