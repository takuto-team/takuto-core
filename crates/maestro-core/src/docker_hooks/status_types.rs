// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Public types for the structured boot-time system-status snapshot.

/// Result of the preflight check. Hard failures (gh, provider) are still returned as `Err`.
/// Soft-fail items (acli) are captured here.
pub struct PreflightResult {
    /// `true` when `acli jira auth status` succeeded.
    pub acli_ok: bool,
}

// ---------------------------------------------------------------------------
// Structured SystemStatus (boot soft-fail)
// ---------------------------------------------------------------------------

/// Snapshot of the deployment's boot-time auth + integration state.
///
/// Source-of-truth shape: `tmp/multi-agents/04_architecture.md §1.2`.
///
/// `collect_system_status` runs every former-hard-error check (gh, provider for
/// the active provider, acli) and returns a populated value **without ever
/// returning `Err`**. Every former hard-error becomes a structured warning with
/// `severity = "critical"` so the dashboard can render a soft-fail banner
/// instead of the binary refusing to boot.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SystemStatus {
    /// `true` when the server got far enough to compute this struct.
    /// (When `config.toml` is missing/unparseable the server does not start at
    /// all; this field is therefore always `true` in any served response.)
    pub config_toml_ok: bool,
    pub github: GitHubStatus,
    pub provider: ProviderStatus,
    pub ticketing: TicketingStatus,
    /// `true` when the SQLite database is initialised — i.e. multi-user auth is
    /// active and per-user credentials are required (vs. legacy single-tenant).
    pub per_user_required: bool,
    pub warnings: Vec<StructuredWarning>,
}

/// GitHub integration state.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GitHubStatus {
    /// `"app"` when a GitHub App is configured; `"pat_required"` when the host
    /// has a personal `gh` auth that workflows can fall back to; `"missing"`
    /// otherwise. The per-user PAT layer (FR-4.2) supplements this.
    pub mode: String,
    pub app_configured: bool,
    pub app_id: Option<u64>,
    pub app_name: Option<String>,
}

/// Provider integration state for the active AI agent (`[agent] provider`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProviderStatus {
    /// `"claude" | "cursor" | "codex" | "opencode" | "none"`. All four
    /// non-none values have runtime adapters.
    pub selected: String,
    /// `true` when a deployment-wide env-var credential is present
    /// (`CLAUDE_CODE_OAUTH_TOKEN` / `CURSOR_API_KEY`).
    pub deployment_default_credential_present: bool,
    /// `true` when the provider can run without a TTY using on-disk credentials
    /// or the deployment-default env var.
    pub headless_capable: bool,
    /// Custom base URL when set (e.g. `ANTHROPIC_BASE_URL`). Returned as-is from
    /// the env var; the value is **never a secret** (URLs only — secrets are
    /// the bearer token, not the endpoint).
    pub custom_base_url: Option<String>,
}

/// Ticketing integration state derived from `[general] ticketing_system`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TicketingStatus {
    /// `"none" | "jira" | "github"`.
    pub system: String,
    /// `true` when `acli jira auth status` succeeded (only meaningful when
    /// `system == "jira"`; always `false` for the other two).
    pub acli_ok: bool,
}

/// A single structured warning. Severity discriminates "must fix before
/// workflows can run" (`critical`) from "advisory" (`warning` / `info`).
///
/// `code` is a short, stable identifier the UI can `switch()` on to render
/// localised copy / setup links. `message` is a human-readable fallback.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StructuredWarning {
    /// e.g. `"gh_auth_missing"`, `"claude_not_authenticated"`,
    /// `"cursor_not_authenticated"`, `"acli_not_authenticated"`.
    pub code: String,
    /// `"critical" | "warning" | "info"`.
    pub severity: String,
    pub message: String,
}

impl StructuredWarning {
    pub(super) fn critical(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            severity: "critical".to_string(),
            message: message.into(),
        }
    }

    pub(super) fn warning(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            severity: "warning".to_string(),
            message: message.into(),
        }
    }

    /// Public `info`-severity constructor used by other crates (currently
    /// `maestro-web`'s `config_agent` handler for the
    /// `config_file_bind_mounted` diagnostic — a non-critical heads-up
    /// that the deployment is on the in-place write fallback). Kept
    /// public so the dashboard refresh path can push without going
    /// through `collect_system_status`.
    pub fn info(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            severity: "info".to_string(),
            message: message.into(),
        }
    }
}

impl SystemStatus {
    /// `true` when any `severity = "critical"` warning is present. The dashboard
    /// uses this to flip into degraded-mode rendering.
    pub fn has_critical(&self) -> bool {
        self.warnings.iter().any(|w| w.severity == "critical")
    }
}

/// Default `SystemStatus` used by tests / fixtures that don't go through
/// `collect_system_status`. All booleans are conservative defaults.
impl Default for SystemStatus {
    fn default() -> Self {
        Self {
            config_toml_ok: true,
            github: GitHubStatus {
                mode: "missing".to_string(),
                app_configured: false,
                app_id: None,
                app_name: None,
            },
            provider: ProviderStatus {
                selected: "claude".to_string(),
                deployment_default_credential_present: false,
                headless_capable: false,
                custom_base_url: None,
            },
            ticketing: TicketingStatus {
                system: "none".to_string(),
                acli_ok: false,
            },
            per_user_required: false,
            warnings: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lock-in: the `SystemStatus::default()` shape feeds the boot-time HTTP
    /// contract consumed by the dashboard. Any field rename / removed default
    /// is an observable wire change; assert the full shape so the split
    /// can't silently drift.
    #[test]
    fn lock_in_system_status_default_shape() {
        let s = SystemStatus::default();

        assert!(s.config_toml_ok, "config_toml_ok default");

        assert_eq!(s.github.mode, "missing");
        assert!(!s.github.app_configured);
        assert_eq!(s.github.app_id, None);
        assert_eq!(s.github.app_name, None);

        assert_eq!(s.provider.selected, "claude");
        assert!(!s.provider.deployment_default_credential_present);
        assert!(!s.provider.headless_capable);
        assert_eq!(s.provider.custom_base_url, None);

        assert_eq!(s.ticketing.system, "none");
        assert!(!s.ticketing.acli_ok);

        assert!(!s.per_user_required);
        assert!(s.warnings.is_empty());

        // has_critical() is false on a default snapshot.
        assert!(!s.has_critical());
    }

    /// Lock-in: the three `StructuredWarning` constructors map to the
    /// `severity` discriminant the dashboard switches on. Any reshuffle of
    /// the constructor → severity wiring is an observable contract break.
    #[test]
    fn lock_in_structured_warning_constructors() {
        let c = StructuredWarning::critical("gh_auth_missing", "gh CLI not authenticated");
        assert_eq!(c.code, "gh_auth_missing");
        assert_eq!(c.severity, "critical");
        assert_eq!(c.message, "gh CLI not authenticated");

        let w = StructuredWarning::warning("acli_not_authenticated", "acli jira auth");
        assert_eq!(w.code, "acli_not_authenticated");
        assert_eq!(w.severity, "warning");
        assert_eq!(w.message, "acli jira auth");

        let i = StructuredWarning::info("config_file_bind_mounted", "in-place fallback");
        assert_eq!(i.code, "config_file_bind_mounted");
        assert_eq!(i.severity, "info");
        assert_eq!(i.message, "in-place fallback");

        // has_critical() flips when any critical warning is present.
        let mut s = SystemStatus::default();
        assert!(!s.has_critical());
        s.warnings.push(StructuredWarning::warning("x", "x"));
        s.warnings.push(StructuredWarning::info("y", "y"));
        assert!(!s.has_critical(), "non-critical warnings must not trip has_critical");
        s.warnings
            .push(StructuredWarning::critical("z", "z"));
        assert!(s.has_critical());
    }
}
