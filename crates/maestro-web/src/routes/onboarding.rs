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

use maestro_core::docker_hooks::{StructuredWarning, SystemStatus};

use crate::state::{AuthState, ConfigState, EngineState};

/// Phase 2b.4: codes that the per-request filter may drop based on the
/// calling user's stored credentials. Anything not on this list — platform
/// issues like `master_key_unavailable`, `secret_key_world_readable`,
/// `config_missing`, `acli_missing`, `provider_not_implemented`, etc. — is
/// preserved as-is because those concern the deployment, not the user.
fn warning_is_user_filterable(code: &str) -> bool {
    matches!(
        code,
        "claude_not_authenticated"
            | "cursor_not_authenticated"
            | "codex_not_authenticated"
            | "opencode_not_authenticated"
            | "gh_auth_missing"
    )
}

/// Map an "active provider" wire string ("claude" / "cursor" / "codex" /
/// "opencode") to the warning code it produces in [`collect_system_status`].
fn provider_warning_code(active_provider: &str) -> Option<&'static str> {
    match active_provider {
        "claude" => Some("claude_not_authenticated"),
        "cursor" => Some("cursor_not_authenticated"),
        "codex" => Some("codex_not_authenticated"),
        "opencode" => Some("opencode_not_authenticated"),
        _ => None,
    }
}

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
    State(auth): State<AuthState>,
    State(cfg_state): State<ConfigState>,
    State(engine): State<EngineState>,
    headers: HeaderMap,
) -> Json<OnboardingStatusBody> {
    let mut status = engine.system_status.read().await.clone();

    let active_provider = status.provider.selected.clone();

    // Phase 2b.4: read the github-app-configured flag from live config (the
    // cached SystemStatus also exposes `github.app_configured`, but reading
    // the config here keeps the rule colocated with the rest of the
    // per-request filter logic and matches the source of truth that
    // `collect_system_status` itself consults).
    let github_app_configured = {
        let cfg = cfg_state.config.read().await;
        cfg.github.is_configured()
    };

    // Phase 2b.4: per-request user resolution. The endpoint is public, so
    // the cookie may be absent — in that case we keep the raw warnings
    // (rule 4: "no filtering possible without a user"). When a cookie IS
    // present and resolves to a user, we additionally:
    //   - drop the active provider's `<p>_not_authenticated` warning if
    //     the user has an active credential row for `p`;
    //   - drop `gh_auth_missing` if the GitHub App is configured (regardless
    //     of user PAT) or if the user has a `user_github_credentials` row;
    //   - drop any non-active provider's `*_not_authenticated` warning
    //     defensively (collect_system_status only emits the active one).
    let (user_onboarding, user_filter): (
        Option<UserOnboardingSummary>,
        Option<UserCredentialState>,
    ) = if let Some(db) = auth.db.as_ref() {
        let cookie = crate::auth::session_cookie_from_headers(&headers)
            .map(|s| s.to_string())
            .unwrap_or_default();
        if cookie.is_empty() {
            (None, None)
        } else {
            // Plan-11 step 3 cluster Sessions: sessions + onboarding +
            // provider_credentials + github_credentials all on the adapter.
            let user_id = crate::auth::validate_db_session(db.adapter(), &cookie).await;

            let pre = if let Some(uid) = user_id {
                let adapter = db.adapter();
                let creds_present = matches!(
                    maestro_core::db::provider_credentials::find_active(
                        adapter,
                        &uid,
                        &active_provider,
                    )
                    .await,
                    Ok(Some(_))
                );
                let github_pat_present = matches!(
                    maestro_core::db::github_credentials::find(adapter, &uid).await,
                    Ok(Some(_))
                );
                Some((uid, creds_present, github_pat_present))
            } else {
                None
            };

            if let Some((user_id, creds_present, github_pat_present)) = pre {
                let row = maestro_core::db::onboarding::get(db.adapter(), &user_id)
                    .await
                    .ok()
                    .flatten();
                let mut summary = match row {
                    Some(r) => UserOnboardingSummary {
                        step_1_ticketing: r.step_1_ticketing.map(|s| s.as_str().to_string()),
                        step_2_provider: r.step_2_provider.map(|s| s.as_str().to_string()),
                        step_3_github: r.step_3_github.map(|s| s.as_str().to_string()),
                        step_4_credentials: r
                            .step_4_credentials
                            .map(|s| s.as_str().to_string()),
                        completed_at: r.completed_at,
                    },
                    None => UserOnboardingSummary::default(),
                };
                if creds_present && summary.step_4_credentials.is_none() {
                    summary.step_4_credentials = Some("completed".to_string());
                }
                (
                    Some(summary),
                    Some(UserCredentialState {
                        active_provider_credential_present: creds_present,
                        github_pat_present,
                    }),
                )
            } else {
                (None, None)
            }
        }
    } else {
        (None, None)
    };

    // Apply the per-user filter to the warning vector. When the caller is
    // unauthenticated (`user_filter = None`), the warnings pass through
    // unchanged — that preserves the public-endpoint contract.
    if let Some(filter) = user_filter.as_ref() {
        status.warnings = apply_user_warning_filter(
            std::mem::take(&mut status.warnings),
            &active_provider,
            github_app_configured,
            filter,
        );
    }

    Json(OnboardingStatusBody {
        status,
        user_onboarding,
    })
}

/// Per-request credential probe; populated from a single DB read so the
/// filter doesn't need to issue queries while the borrow is live.
struct UserCredentialState {
    active_provider_credential_present: bool,
    github_pat_present: bool,
}

/// Drop warnings that no longer apply once we know who the caller is.
///
/// Rules (per task #30):
///   1. Drop `<active_provider>_not_authenticated` when the user has an
///      active credential for the active provider.
///   2. Drop ALL non-active provider `*_not_authenticated` warnings
///      defensively. (`collect_system_status` only emits the active
///      provider's warning today, but this guards against future drift.)
///   3. Drop `gh_auth_missing` when the GitHub App is configured OR the
///      user has a `user_github_credentials` row.
///   4. Keep platform warnings (`master_key_unavailable`,
///      `secret_key_world_readable`, `config_missing`, `acli_missing`,
///      `provider_not_implemented`) unchanged.
fn apply_user_warning_filter(
    warnings: Vec<StructuredWarning>,
    active_provider: &str,
    github_app_configured: bool,
    user: &UserCredentialState,
) -> Vec<StructuredWarning> {
    let active_warning_code = provider_warning_code(active_provider);
    warnings
        .into_iter()
        .filter(|w| {
            if !warning_is_user_filterable(&w.code) {
                return true; // Platform / non-user warning: keep.
            }
            match w.code.as_str() {
                "gh_auth_missing" => {
                    // Keep only when BOTH App and user PAT are absent.
                    !(github_app_configured || user.github_pat_present)
                }
                code if Some(code) == active_warning_code => {
                    // Active provider: drop if user has the credential.
                    !user.active_provider_credential_present
                }
                _ => {
                    // Non-active provider warning: defensively drop.
                    false
                }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn critical(code: &str) -> StructuredWarning {
        StructuredWarning {
            code: code.into(),
            severity: "critical".into(),
            message: format!("{code} fixture"),
        }
    }

    fn user_state(provider_cred: bool, github_pat: bool) -> UserCredentialState {
        UserCredentialState {
            active_provider_credential_present: provider_cred,
            github_pat_present: github_pat,
        }
    }

    fn codes(ws: &[StructuredWarning]) -> Vec<&str> {
        ws.iter().map(|w| w.code.as_str()).collect()
    }

    /// Rule 1: active provider's warning is dropped when the user has the
    /// matching credential.
    #[test]
    fn active_provider_warning_dropped_when_user_has_credential() {
        let out = apply_user_warning_filter(
            vec![critical("claude_not_authenticated")],
            "claude",
            false,
            &user_state(true, false),
        );
        assert!(out.is_empty());
    }

    /// Rule 1, counter: warning stays when the user has no credential.
    #[test]
    fn active_provider_warning_kept_when_user_has_no_credential() {
        let out = apply_user_warning_filter(
            vec![critical("claude_not_authenticated")],
            "claude",
            false,
            &user_state(false, false),
        );
        assert_eq!(codes(&out), vec!["claude_not_authenticated"]);
    }

    /// Rule 2: non-active provider warnings are dropped defensively even
    /// when the user lacks credentials for them.
    #[test]
    fn non_active_provider_warning_dropped_defensively() {
        // active = cursor, system somehow emits claude warning too.
        let out = apply_user_warning_filter(
            vec![
                critical("cursor_not_authenticated"),
                critical("claude_not_authenticated"),
            ],
            "cursor",
            false,
            &user_state(false, false),
        );
        // cursor warning stays (no cursor cred); claude warning dropped.
        assert_eq!(codes(&out), vec!["cursor_not_authenticated"]);
    }

    /// Rule 3a: `gh_auth_missing` dropped when the GitHub App is configured.
    #[test]
    fn gh_auth_missing_dropped_when_app_configured() {
        let out = apply_user_warning_filter(
            vec![critical("gh_auth_missing")],
            "claude",
            true, // App configured
            &user_state(false, false),
        );
        assert!(out.is_empty());
    }

    /// Rule 3b: `gh_auth_missing` dropped when the user has a PAT row.
    #[test]
    fn gh_auth_missing_dropped_when_user_has_pat() {
        let out = apply_user_warning_filter(
            vec![critical("gh_auth_missing")],
            "claude",
            false,
            &user_state(false, true),
        );
        assert!(out.is_empty());
    }

    /// Rule 3c: `gh_auth_missing` kept when neither App nor user PAT.
    #[test]
    fn gh_auth_missing_kept_when_neither_app_nor_pat() {
        let out = apply_user_warning_filter(
            vec![critical("gh_auth_missing")],
            "claude",
            false,
            &user_state(false, false),
        );
        assert_eq!(codes(&out), vec!["gh_auth_missing"]);
    }

    /// Rule 4: platform warnings pass through unchanged.
    #[test]
    fn platform_warnings_pass_through_unchanged() {
        let inputs: Vec<StructuredWarning> = [
            "master_key_unavailable",
            "secret_key_world_readable",
            "config_missing",
            "acli_missing",
            "acli_not_authenticated",
            "provider_not_implemented",
            "cursor_cli_missing",
        ]
        .into_iter()
        .map(critical)
        .collect();
        let out = apply_user_warning_filter(
            inputs.clone(),
            "claude",
            false,
            &user_state(false, false),
        );
        assert_eq!(out.len(), inputs.len(), "every platform warning must survive");
        assert_eq!(codes(&out), codes(&inputs));
    }

    /// Compound: active provider Claude with credential + App configured +
    /// every platform warning + a stray cursor warning → only the platform
    /// warnings remain.
    #[test]
    fn compound_user_with_creds_and_app_keeps_only_platform_warnings() {
        let out = apply_user_warning_filter(
            vec![
                critical("claude_not_authenticated"),
                critical("cursor_not_authenticated"),
                critical("gh_auth_missing"),
                critical("master_key_unavailable"),
                critical("acli_not_authenticated"),
            ],
            "claude",
            true,
            &user_state(true, false),
        );
        assert_eq!(
            codes(&out),
            vec!["master_key_unavailable", "acli_not_authenticated"],
            "only platform warnings should survive a fully-set-up user"
        );
    }

    #[test]
    fn provider_warning_code_mapping_is_complete_and_stable() {
        assert_eq!(provider_warning_code("claude"), Some("claude_not_authenticated"));
        assert_eq!(provider_warning_code("cursor"), Some("cursor_not_authenticated"));
        assert_eq!(provider_warning_code("codex"), Some("codex_not_authenticated"));
        assert_eq!(
            provider_warning_code("opencode"),
            Some("opencode_not_authenticated")
        );
        assert_eq!(provider_warning_code("anything_else"), None);
    }
}
