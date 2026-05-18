// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Phase 1: `PUT /api/config/agent` — admin-only patch endpoint for the
//! `[agent]` section. Source: 04_architecture.md §2.3.
//!
//! Why a new endpoint instead of extending `PUT /api/config`: the existing
//! patch is a strict 4-field allowlist (web.{user,password},
//! general.{max_concurrent_workflows,max_active_workflows}). Mixing the richer
//! agent surface into that schema bloats `RuntimeDashboardConfigPatch` and
//! makes the strict `deny_unknown_fields` contract harder to evolve.

use axum::Json;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use chrono::Utc;
use serde::Deserialize;
use tracing::{info, warn};

use maestro_core::config::{
    AgentProviderConfig, AiAgentProvider, CodexProviderConfig, CursorProviderConfig,
    validate_extra_args,
};
use maestro_core::docker_hooks::collect_system_status_with_db;
use maestro_core::workflow::engine::WorkflowEvent;

use crate::auth::AuthenticatedUser;
use crate::routes::admin::require_admin_for;
use crate::routes::config::UpdateConfigResponse;
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Request bodies — every field optional, deny_unknown_fields throughout.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutAgentConfigRequest {
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub available_providers: Option<Vec<String>>,
    #[serde(default)]
    pub providers: Option<ProvidersPatch>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProvidersPatch {
    #[serde(default)]
    pub claude: Option<AgentProviderPatch>,
    #[serde(default)]
    pub cursor: Option<CursorProviderPatch>,
    #[serde(default)]
    pub codex: Option<CodexProviderPatch>,
    #[serde(default)]
    pub opencode: Option<AgentProviderPatch>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentProviderPatch {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub extra_args: Option<Vec<String>>,
    #[serde(default)]
    pub allow_shared_default: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CursorProviderPatch {
    #[serde(default)]
    pub cli: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub extra_args: Option<Vec<String>>,
    #[serde(default)]
    pub allow_shared_default: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodexProviderPatch {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub provider_name: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub extra_args: Option<Vec<String>>,
    #[serde(default)]
    pub allow_shared_default: Option<bool>,
}

// ---------------------------------------------------------------------------
// Patch application
// ---------------------------------------------------------------------------

fn apply_generic_patch(target: &mut AgentProviderConfig, patch: AgentProviderPatch) {
    if let Some(v) = patch.model {
        target.model = v;
    }
    if let Some(v) = patch.base_url {
        target.base_url = v;
    }
    if let Some(v) = patch.extra_args {
        target.extra_args = v;
    }
    if let Some(v) = patch.allow_shared_default {
        target.allow_shared_default = v;
    }
}

fn apply_cursor_patch(target: &mut CursorProviderConfig, patch: CursorProviderPatch) {
    if let Some(v) = patch.cli {
        target.cli = v;
    }
    if let Some(v) = patch.model {
        target.model = v;
    }
    if let Some(v) = patch.extra_args {
        target.extra_args = v;
    }
    if let Some(v) = patch.allow_shared_default {
        target.allow_shared_default = v;
    }
}

fn apply_codex_patch(target: &mut CodexProviderConfig, patch: CodexProviderPatch) {
    if let Some(v) = patch.model {
        target.model = v;
    }
    if let Some(v) = patch.provider_name {
        target.provider_name = v;
    }
    if let Some(v) = patch.base_url {
        target.base_url = v;
    }
    if let Some(v) = patch.extra_args {
        target.extra_args = v;
    }
    if let Some(v) = patch.allow_shared_default {
        target.allow_shared_default = v;
    }
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// `PUT /api/config/agent` — admin-only patch of the `[agent]` section.
///
/// Steps (matches 04_architecture.md §2.3):
/// 1. `require_admin_for` — 403 for non-admin.
/// 2. Pre-validate `extra_args` against the deny-list before grabbing the lock.
/// 3. Apply patch under `state.config.write().await`. Validator failure → 400.
/// 4. Clone + drop the lock, then persist via `ConfigWriter::write_config`.
/// 5. Refresh `state.system_status` from the patched config so the
///    dashboard's next `/api/auth/status` / `/api/onboarding/status` call
///    reflects the new provider / degraded state without a process restart.
/// 6. If the active provider changed, broadcast a `provider_changed`
///    WebSocket event so every connected dashboard re-renders the banner.
///
/// Why `Json<serde_json::Value>` instead of `Json<PutAgentConfigRequest>`:
/// Axum's typed extractor maps `serde(deny_unknown_fields)` rejection to
/// **422 Unprocessable Entity**, but the QA matrix and the documented
/// convention for `PUT /api/config` is **400 Bad Request**. We accept the
/// raw value first, then deserialize manually so the 400 path stays in
/// control of the handler.
pub async fn put_agent_config(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Json(raw): Json<serde_json::Value>,
) -> Result<Json<UpdateConfigResponse>, (StatusCode, String)> {
    require_admin_for(&state, &auth)
        .await
        .map_err(|s| (s, String::new()))?;

    // Parse with `deny_unknown_fields` ourselves so we can map serde errors
    // to 400 (matches PUT /api/config) instead of axum's default 422.
    let patch: PutAgentConfigRequest = serde_json::from_value(raw).map_err(|e| {
        let body = serde_json::json!({
            "error": "unknown_field_or_invalid_shape",
            "detail": e.to_string(),
        })
        .to_string();
        (StatusCode::BAD_REQUEST, body)
    })?;

    // Pre-validate the patch's `extra_args` so we surface the typed
    // `extra_args_denied` error before grabbing the write lock.
    if let Some(ref providers) = patch.providers {
        if let Some(ref p) = providers.claude
            && let Some(ref ea) = p.extra_args
        {
            validate_extra_args(ea).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        }
        if let Some(ref p) = providers.cursor
            && let Some(ref ea) = p.extra_args
        {
            validate_extra_args(ea).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        }
        if let Some(ref p) = providers.codex
            && let Some(ref ea) = p.extra_args
        {
            validate_extra_args(ea).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        }
        if let Some(ref p) = providers.opencode
            && let Some(ref ea) = p.extra_args
        {
            validate_extra_args(ea).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        }
    }

    // Pre-validate the provider string so we 400 before locking.
    if let Some(ref p) = patch.provider {
        AiAgentProvider::parse(p).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
    }
    if let Some(ref list) = patch.available_providers {
        for p in list {
            AiAgentProvider::parse(p).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        }
    }

    // Apply under write lock, then clone + release before any I/O.
    let (config_snapshot, provider_change) = {
        let mut config = state.config.write().await;
        let previous_provider = config.agent.provider.as_str().to_string();

        if let Some(provider_str) = patch.provider {
            // parse() already validated above; unwrap is safe here.
            let p = AiAgentProvider::parse(&provider_str)
                .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
            config.agent.provider = p;
        }
        if let Some(list) = patch.available_providers {
            config.agent.available_providers = list;
        }
        if let Some(providers_patch) = patch.providers {
            if let Some(p) = providers_patch.claude {
                apply_generic_patch(&mut config.agent.providers.claude, p);
            }
            if let Some(p) = providers_patch.cursor {
                apply_cursor_patch(&mut config.agent.providers.cursor, p);
            }
            if let Some(p) = providers_patch.codex {
                apply_codex_patch(&mut config.agent.providers.codex, p);
            }
            if let Some(p) = providers_patch.opencode {
                apply_generic_patch(&mut config.agent.providers.opencode, p);
            }
        }

        // Re-validate the resulting config (catches denied extra_args set on
        // a sub-table the request did not touch, and any other invariant).
        config
            .validate()
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

        let new_provider = config.agent.provider.as_str().to_string();
        let change = if previous_provider != new_provider {
            Some((previous_provider, new_provider))
        } else {
            None
        };
        (config.clone(), change)
    };

    // Persist to disk OUTSIDE the lock.
    let (persisted, persist_warning) = if let Some(ref writer) = state.config_writer {
        match writer.write_config(&config_snapshot) {
            Ok(()) => (true, None),
            Err(e) => {
                warn!(
                    error = %e,
                    "[agent] config patched in memory but disk write failed"
                );
                (false, Some(e.to_string()))
            }
        }
    } else {
        (false, None)
    };

    // Phase 1 AC-4: refresh `state.system_status` so subsequent reads of
    // `/api/onboarding/status` and the three mirrored fields on
    // `/api/auth/status` reflect the new provider / degraded state without
    // a process restart. We do this regardless of whether the disk write
    // succeeded — the in-memory config was applied and validated either way,
    // and the dashboard polls these endpoints on the WS `provider_changed`
    // event we're about to broadcast.
    // Phase 2a: pass the DB through so master-key warnings (which depend on
    // boot-time key resolution, not the patched config) are re-attached.
    let refreshed = collect_system_status_with_db(&config_snapshot, state.db.as_ref());
    {
        let mut s = state.system_status.write().await;
        // Preserve per_user_required from the existing snapshot — it tracks
        // DB availability (set once at boot in run_server), not anything
        // collect_system_status sees from a Config alone.
        let prior_per_user_required = s.per_user_required;
        *s = refreshed;
        s.per_user_required = prior_per_user_required;
    }

    // Broadcast provider.changed (Phase 1: affected_users is empty until
    // Phase 2 ships per-user credentials).
    if let Some((from, to)) = provider_change {
        info!(from = %from, to = %to, "Active AI provider changed via PUT /api/config/agent");
        state.engine.broadcast_event(WorkflowEvent {
            event_type: "provider_changed".to_string(),
            workflow_id: String::new(),
            ticket_key: String::new(),
            state: String::new(),
            timestamp: Utc::now(),
            provider_from: Some(from),
            provider_to: Some(to),
            affected_users: Some(Vec::new()),
            ..Default::default()
        });
    }

    Ok(Json(UpdateConfigResponse {
        config: config_snapshot.redacted_for_api_clone(),
        persisted,
        persist_warning,
    }))
}
