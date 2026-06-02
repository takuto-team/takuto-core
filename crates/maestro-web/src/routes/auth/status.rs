// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! `GET /api/auth/status` — public auth/setup probe.

use axum::Json;
use axum::extract::State;
use serde::Serialize;

use crate::state::{AuthState, EngineState};

#[derive(Debug, Serialize)]
pub struct AuthStatus {
    pub dashboard_auth_enabled: bool,
    /// `true` when the SQLite database has users (multi-user mode active).
    pub multi_user: bool,
    /// `true` when the database is available but has no users yet (first-user registration required).
    pub setup_required: bool,
    /// Mirror of `system_status.provider.selected` so the login page can
    /// render the right provider-specific hint without a second round-trip.
    pub provider_selected: String,
    /// Mirror of `system_status.github.mode`.
    pub github_mode: String,
    /// `true` when any critical warning exists in `system_status`. The
    /// dashboard uses this to render the degraded-mode banner.
    pub degraded: bool,
    /// `true` when the caller is authenticated AND has an active provider
    /// credential row for the deployment-wide active provider. `false`
    /// for unauthenticated callers and for users who haven't pasted any
    /// credential yet. Drives the per-user "set up your provider" banner
    /// on the dashboard.
    pub provider_credential_present: bool,
}

/// Public probe: whether the server requires dashboard login.
pub async fn auth_status(
    State(auth): State<AuthState>,
    State(engine): State<EngineState>,
    headers: axum::http::HeaderMap,
) -> Json<AuthStatus> {
    // system_status is mutable (refreshed after PUT /api/config/agent),
    // so take a snapshot under the read lock and drop it before any
    // other awaits.
    let (provider_selected, github_mode, degraded) = {
        let s = engine.system_status.read().await;
        (
            s.provider.selected.clone(),
            s.github.mode.clone(),
            s.has_critical(),
        )
    };

    // Optionally resolve the caller's identity to surface their per-user
    // credential state. The endpoint stays public (no auth gate); an
    // unauthenticated request reports `provider_credential_present:
    // false` and the rest of the fields exactly as before.
    let provider_credential_present = if let Some(ref db) = auth.db {
        let active_provider = provider_selected.clone();
        let cookie = crate::auth::session_cookie_from_headers(&headers)
            .map(|s| s.to_string())
            .unwrap_or_default();
        if cookie.is_empty() {
            false
        } else {
            let adapter = db.adapter();
            let user_id = crate::auth::validate_db_session(adapter, &cookie).await;
            match user_id {
                Some(uid) => matches!(
                    maestro_core::db::provider_credentials::find_active(
                        adapter,
                        &uid,
                        &active_provider,
                    )
                    .await,
                    Ok(Some(_))
                ),
                None => false,
            }
        }
    } else {
        false
    };

    if let Some(ref db) = auth.db {
        let count = maestro_core::db::users::count_users(db.adapter())
            .await
            .unwrap_or(0);
        return Json(AuthStatus {
            dashboard_auth_enabled: true,
            multi_user: count > 0,
            setup_required: count == 0,
            provider_selected,
            github_mode,
            degraded,
            provider_credential_present,
        });
    }

    // No database — auth is required but setup cannot proceed.
    // The UI will show setup_required and the user must fix the data directory.
    Json(AuthStatus {
        dashboard_auth_enabled: true,
        multi_user: false,
        setup_required: true,
        provider_selected,
        github_mode,
        degraded,
        provider_credential_present,
    })
}
