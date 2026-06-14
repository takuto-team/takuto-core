// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! `PUT /api/config/agent` — admin-only patch endpoint for the `[agent]`
//! section. Source: 04_architecture.md §2.3.
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
    OpenCodeProviderConfig, validate_extra_args,
};
use maestro_core::docker_hooks::collect_system_status_with_db;
use maestro_core::workflow::engine::WorkflowEvent;

use crate::auth::AuthenticatedUser;
use crate::routes::admin::require_admin_for;
use crate::routes::config::UpdateConfigResponse;
use crate::state::{AuthState, ConfigState, EngineState};

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
    /// Share one agent conversation across all steps in a flow (each step
    /// resumes the prior step's session) vs. a fresh session per step.
    #[serde(default)]
    pub share_conversation_across_steps: Option<bool>,
    /// Timeout per agent session, in seconds. `Config::validate` enforces the
    /// `>= 1` floor.
    #[serde(default)]
    pub step_timeout_secs: Option<u64>,
    /// Timeout for "Improve with AI" / "Prompt ticket" sessions, in seconds.
    #[serde(default)]
    pub improve_timeout_secs: Option<u64>,
    /// No-progress guardrail: abort a step after this many consecutive
    /// repeated output lines. `0` disables the guardrail.
    #[serde(default)]
    pub max_repeated_output_lines: Option<u32>,
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
    pub opencode: Option<OpenCodeProviderPatch>,
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

/// OpenCode patch — the generic fields plus the two self-hosted-only token
/// limits. The limits use a "double option" so the request can express three
/// distinct intents: absent (leave alone), `null` (clear back to default), or
/// a number (set). A plain `Option<u32>` cannot distinguish absent from
/// `null`, which the dashboard form needs to clear a previously-set limit.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenCodeProviderPatch {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub extra_args: Option<Vec<String>>,
    #[serde(default)]
    pub allow_shared_default: Option<bool>,
    #[serde(default, deserialize_with = "double_option")]
    pub context_limit: Option<Option<u32>>,
    #[serde(default, deserialize_with = "double_option")]
    pub output_limit: Option<Option<u32>>,
}

/// Deserialize a present-but-maybe-null field into `Some(inner)` so callers
/// can tell "key omitted" (`None`) from "key explicitly null" (`Some(None)`).
// The three-way distinction is the whole point here, so the nested Option is
// intentional rather than a smell.
#[allow(clippy::option_option)]
fn double_option<'de, D, T>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Some(Option::deserialize(de)?))
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

fn apply_opencode_patch(target: &mut OpenCodeProviderConfig, patch: OpenCodeProviderPatch) {
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
    // `Some(inner)` = field was present (inner may be None to clear);
    // outer `None` = field omitted, leave the stored value untouched.
    if let Some(v) = patch.context_limit {
        target.context_limit = v;
    }
    if let Some(v) = patch.output_limit {
        target.output_limit = v;
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
/// 3. Apply patch under `state.config.config.write().await`. Validator failure → 400.
/// 4. Clone + drop the lock, then persist via `ConfigWriter::write_config`.
/// 5. Refresh `state.engine.system_status` from the patched config so the
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
    State(auth_state): State<AuthState>,
    State(cfg_state): State<ConfigState>,
    State(engine): State<EngineState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Json(raw): Json<serde_json::Value>,
) -> Result<Json<UpdateConfigResponse>, (StatusCode, String)> {
    require_admin_for(&auth_state, &auth)
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
        let mut config = cfg_state.config.write().await;
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
        if let Some(share) = patch.share_conversation_across_steps {
            config.agent.share_conversation_across_steps = share;
        }
        if let Some(v) = patch.step_timeout_secs {
            config.agent.step_timeout_secs = v;
        }
        if let Some(v) = patch.improve_timeout_secs {
            config.agent.improve_timeout_secs = v;
        }
        if let Some(v) = patch.max_repeated_output_lines {
            config.agent.max_repeated_output_lines = v;
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
                apply_opencode_patch(&mut config.agent.providers.opencode, p);
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
    let (persisted, persist_warning) = if let Some(ref writer) = cfg_state.config_writer {
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

    // Refresh `state.engine.system_status` so subsequent reads of
    // `/api/onboarding/status` and the three mirrored fields on
    // `/api/auth/status` reflect the new provider / degraded state without
    // a process restart. We do this regardless of whether the disk write
    // succeeded — the in-memory config was applied and validated either way,
    // and the dashboard polls these endpoints on the WS `provider_changed`
    // event we're about to broadcast. Pass the DB through so master-key
    // warnings (which depend on boot-time key resolution, not the patched
    // config) are re-attached.
    let mut refreshed = collect_system_status_with_db(&config_snapshot, auth_state.db.as_ref());
    // Also re-probe config-dir write-ability on the refresh path. The
    // dashboard typically triggers this branch via PUT /api/config/agent,
    // so the user just attempted a save — surfacing the warning here means
    // the next poll of /api/onboarding/status immediately exposes "your
    // save didn't persist" without waiting for a restart.
    if let Some(w) = maestro_core::docker_hooks::check_config_dir_writable(&cfg_state.config_path) {
        refreshed.warnings.push(w);
    }
    // When the writer has had to fall back to the in-place path because
    // `config.toml` is bind-mounted as a single file, surface an
    // info-level diagnostic so admins know which write protocol is
    // active. The flag latches `true` for the process lifetime — emit
    // once-set-stay-set semantics. Severity is `info` (not critical)
    // because saves SUCCEED via the fallback; this is purely a "you're on
    // the alt path" notice, not a failure.
    if let Some(ref writer) = cfg_state.config_writer
        && writer
            .used_inplace_fallback()
            .load(std::sync::atomic::Ordering::Acquire)
    {
        refreshed
            .warnings
            .push(maestro_core::docker_hooks::StructuredWarning::info(
                "config_file_bind_mounted",
                "config.toml is bind-mounted as a single file. Dashboard saves \
                 use in-place writes (atomic rename is unsupported on this \
                 mount layout). Saves continue to work; this is informational.",
            ));
    }
    {
        let mut s = engine.system_status.write().await;
        // Preserve per_user_required from the existing snapshot — it tracks
        // DB availability (set once at boot in run_server), not anything
        // collect_system_status sees from a Config alone.
        let prior_per_user_required = s.per_user_required;
        *s = refreshed;
        s.per_user_required = prior_per_user_required;
    }

    // Broadcast provider.changed. `affected_users` is empty until per-user
    // credentials are wired through this event.
    if let Some((from, to)) = provider_change {
        info!(from = %from, to = %to, "Active AI provider changed via PUT /api/config/agent");
        engine.engine.broadcast_event(WorkflowEvent {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The OpenCode limits are tri-state: absent (leave alone), `null`
    /// (clear), or a number (set). `double_option` is what makes the first
    /// two distinguishable.
    #[test]
    fn opencode_limit_double_option_distinguishes_absent_null_and_value() {
        // Absent → outer None (leave alone).
        let p: OpenCodeProviderPatch = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(p.context_limit, None);
        assert_eq!(p.output_limit, None);

        // Explicit null → Some(None) (clear).
        let p: OpenCodeProviderPatch =
            serde_json::from_value(serde_json::json!({ "context_limit": null })).unwrap();
        assert_eq!(p.context_limit, Some(None));

        // Number → Some(Some(n)) (set).
        let p: OpenCodeProviderPatch =
            serde_json::from_value(serde_json::json!({ "context_limit": 32768 })).unwrap();
        assert_eq!(p.context_limit, Some(Some(32768)));
    }

    #[test]
    fn apply_opencode_patch_sets_clears_and_leaves_limits() {
        let mut cfg = OpenCodeProviderConfig {
            context_limit: Some(8192),
            output_limit: Some(4096),
            ..Default::default()
        };

        // Omitted → unchanged; set output to a new value.
        apply_opencode_patch(
            &mut cfg,
            OpenCodeProviderPatch {
                model: None,
                base_url: None,
                extra_args: None,
                allow_shared_default: None,
                context_limit: None,
                output_limit: Some(Some(16000)),
            },
        );
        assert_eq!(
            cfg.context_limit,
            Some(8192),
            "omitted must leave unchanged"
        );
        assert_eq!(cfg.output_limit, Some(16000), "Some(Some) must set");

        // Explicit null clears.
        apply_opencode_patch(
            &mut cfg,
            OpenCodeProviderPatch {
                model: None,
                base_url: None,
                extra_args: None,
                allow_shared_default: None,
                context_limit: Some(None),
                output_limit: None,
            },
        );
        assert_eq!(cfg.context_limit, None, "Some(None) must clear");
        assert_eq!(cfg.output_limit, Some(16000), "untouched on this call");
    }

    /// Unknown keys in the opencode patch are rejected (deny_unknown_fields).
    #[test]
    fn opencode_patch_rejects_unknown_fields() {
        let r: Result<OpenCodeProviderPatch, _> =
            serde_json::from_value(serde_json::json!({ "bogus": 1 }));
        assert!(r.is_err());
    }

    /// The step-guardrail fields parse into the typed request body.
    #[test]
    fn step_guardrail_fields_parse() {
        let p: PutAgentConfigRequest = serde_json::from_value(serde_json::json!({
            "step_timeout_secs": 600,
            "improve_timeout_secs": 120,
            "max_repeated_output_lines": 0,
        }))
        .unwrap();
        assert_eq!(p.step_timeout_secs, Some(600));
        assert_eq!(p.improve_timeout_secs, Some(120));
        assert_eq!(p.max_repeated_output_lines, Some(0));
    }
}

#[cfg(test)]
mod http_tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::server::build_router;
    use crate::state::AppState;
    use crate::test_helpers::{TEST_ORIGIN, register_and_login, test_state_with_db};

    async fn create_and_login_user(state: &AppState, admin_cookie: &str) -> String {
        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::post("/api/users")
                    .header("Content-Type", "application/json")
                    .header("Origin", TEST_ORIGIN)
                    .header("Cookie", admin_cookie)
                    .body(Body::from(
                        r#"{"username":"viewer","password":"viewerpass1234"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        let app = build_router(state.clone());
        let login = app
            .oneshot(
                Request::post("/api/auth/login")
                    .header("Content-Type", "application/json")
                    .header("Origin", TEST_ORIGIN)
                    .body(Body::from(
                        r#"{"username":"viewer","password":"viewerpass1234"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(login.status(), StatusCode::NO_CONTENT);
        let set_cookie = login
            .headers()
            .get("set-cookie")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        set_cookie.split(';').next().unwrap().trim().to_string()
    }

    #[tokio::test]
    async fn put_agent_step_guardrails_persist() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::put("/api/config/agent")
                    .header("Content-Type", "application/json")
                    .header("Origin", TEST_ORIGIN)
                    .header("Cookie", &cookie)
                    .body(Body::from(
                        r#"{"step_timeout_secs":900,"improve_timeout_secs":240,"max_repeated_output_lines":12}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["agent"]["step_timeout_secs"], 900);
        assert_eq!(json["agent"]["improve_timeout_secs"], 240);
        assert_eq!(json["agent"]["max_repeated_output_lines"], 12);

        let cfg = state.config.config.read().await;
        assert_eq!(cfg.agent.step_timeout_secs, 900);
        assert_eq!(cfg.agent.improve_timeout_secs, 240);
        assert_eq!(cfg.agent.max_repeated_output_lines, 12);
    }

    #[tokio::test]
    async fn put_agent_step_timeout_zero_returns_400() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::put("/api/config/agent")
                    .header("Content-Type", "application/json")
                    .header("Origin", TEST_ORIGIN)
                    .header("Cookie", &cookie)
                    .body(Body::from(r#"{"step_timeout_secs":0}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn put_agent_unknown_field_returns_400() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::put("/api/config/agent")
                    .header("Content-Type", "application/json")
                    .header("Origin", TEST_ORIGIN)
                    .header("Cookie", &cookie)
                    .body(Body::from(r#"{"bogus":true}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn put_agent_non_admin_returns_403() {
        let state = test_state_with_db();
        let admin_cookie = register_and_login(&state).await;
        let user_cookie = create_and_login_user(&state, &admin_cookie).await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::put("/api/config/agent")
                    .header("Content-Type", "application/json")
                    .header("Origin", TEST_ORIGIN)
                    .header("Cookie", &user_cookie)
                    .body(Body::from(r#"{"step_timeout_secs":600}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }
}
