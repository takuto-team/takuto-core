// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Config-driven shell hooks for Docker image build and container startup.

mod cursor_auth;
mod gh_auth;
mod hook_runner;
mod process;
mod status_types;

pub use hook_runner::run_hook_commands;
pub use status_types::{
    GitHubStatus, PreflightResult, ProviderStatus, StructuredWarning, SystemStatus,
    TicketingStatus,
};

use crate::config::{AiAgentProvider, Config, TicketingSystem};
use crate::error::{MaestroError, Result};
use cursor_auth::cursor_agent_auth_likely_on_disk;
use gh_auth::gh_auth_recover_expired_token;
use process::auth_cmd_ok;

/// Collect a structured `SystemStatus` snapshot. **Never returns `Err`** —
/// every former hard error becomes a `severity = "critical"` warning. This is
/// the Phase 0 replacement for `preflight()` and is what the dashboard reads
/// via `GET /api/onboarding/status`.
///
/// Phase 2a (04_architecture.md §3.2): when a `Database` handle is provided,
/// the helper also surfaces master-key warnings (`master_key_unavailable` and
/// `secret_key_world_readable`) so the dashboard can render the degraded-mode
/// banner before any credential CRUD endpoint is hit. Callers that don't have
/// a DB in scope (e.g. the standalone `maestro preflight` CLI subcommand)
/// pass `None` and get config-only warnings.
pub fn collect_system_status(config: &Config) -> SystemStatus {
    collect_system_status_with_db(config, None)
}

/// Task #37 (Phase 2c, deployment fix): probe the config directory for
/// write-ability and return a `config_dir_not_writable` warning when the
/// process can't atomically create a file there.
///
/// `ConfigWriter::write_atomic` writes a `.tmp` sibling then `rename(2)`s
/// it over `config.toml`. POSIX `rename(2)` requires write on the *parent
/// directory*, not the target file — so if the bind-mounted `/etc/maestro/`
/// is root-owned and the runtime user is `maestro`, every dashboard save
/// (PUT /api/config/agent) succeeds in-memory but fails to persist with
/// `Permission denied (os error 13)`. The user sees a "saved" toast (until
/// the persist_warning UI surfaces it), restarts the container, the change
/// is gone.
///
/// We emit the warning at **critical** severity so it joins the platform
/// warnings that survive `apply_user_warning_filter` and stays visible to
/// every authenticated caller. The deployment-level entrypoint chown in
/// `docker/entrypoint.sh` is the canonical fix; this warning catches the
/// case where the chown silently failed (read-only mount, non-root
/// container start, …).
///
/// Behaviour:
///   - Parent dir missing → returns `None` (no double-warn; the missing
///     config is already covered by `config_missing` elsewhere).
///   - Probe succeeds → returns `None`.
///   - Probe fails with `PermissionDenied` → returns the warning.
///   - Probe fails with any other I/O error → returns `None` so we don't
///     flag transient errors (e.g. EROFS from a CI sanity-check mount).
pub fn check_config_dir_writable(config_path: &std::path::Path) -> Option<StructuredWarning> {
    let dir = config_path.parent()?;
    if !dir.exists() {
        return None;
    }
    match tempfile::Builder::new()
        .prefix(".maestro-write-probe-")
        .tempfile_in(dir)
    {
        Ok(_) => None,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            Some(StructuredWarning::critical(
                "config_dir_not_writable",
                format!(
                    "Maestro can't write to the config directory ({}). Dashboard \
                     configuration changes will not persist across restarts. \
                     Restart Maestro after fixing directory permissions.",
                    dir.display()
                ),
            ))
        }
        Err(_) => None,
    }
}

/// Like [`collect_system_status`] but additionally emits Phase 2a master-key
/// warnings derived from the database's master-key state.
pub fn collect_system_status_with_db(
    config: &Config,
    db: Option<&crate::db::Database>,
) -> SystemStatus {
    let mut warnings: Vec<StructuredWarning> = Vec::new();

    // Phase 2a: when a DB handle is provided, emit master-key warnings.
    // These come first so the dashboard can render them at the top of the
    // banner — they block per-user credential CRUD entirely.
    if let Some(db) = db {
        match db.master_key() {
            None => {
                warnings.push(StructuredWarning::critical(
                    "master_key_unavailable",
                    "Master key unavailable: set MAESTRO_SECRET_KEY or enable [general] allow_auto_generate_secret_key. Per-user credential storage is disabled until this is resolved.",
                ));
            }
            Some(state) if state.keyfile_world_readable => {
                warnings.push(StructuredWarning::critical(
                    "secret_key_world_readable",
                    "Master keyfile permissions are not 0600. Re-secure with `chmod 600 ${data_dir}/secret.key` (cold-disk leak risk).",
                ));
            }
            Some(_) => {}
        }
    }

    // ── GitHub ────────────────────────────────────────────────────────────
    let github = if config.github.is_configured() {
        GitHubStatus {
            mode: "app".to_string(),
            app_configured: true,
            app_id: Some(config.github.app_id),
            app_name: if config.github.app_name.trim().is_empty() {
                None
            } else {
                Some(config.github.app_name.clone())
            },
        }
    } else {
        // No App configured — fall back to host `gh` auth. The presence/validity
        // of that auth is informational at this layer (Phase 2 introduces the
        // per-user PAT). When the active host token is invalid we surface a
        // critical warning instead of returning Err.
        let token_exists = auth_cmd_ok("gh", &["auth", "token", "-h", "github.com"]);
        let mut token_valid = token_exists && auth_cmd_ok("gh", &["api", "user"]);
        // Recovery: if the active user has an expired token (common with GitHub
        // App installation tokens that expire hourly), try switching to a user
        // with a personal token before reporting "missing".
        if !token_valid && gh_auth_recover_expired_token() {
            token_valid = true;
        }
        if token_valid {
            GitHubStatus {
                mode: "pat_required".to_string(),
                app_configured: false,
                app_id: None,
                app_name: None,
            }
        } else {
            warnings.push(StructuredWarning::critical(
                "gh_auth_missing",
                "GitHub authentication is not configured. Either provision a GitHub App in [github] or authenticate `gh` on the host.",
            ));
            GitHubStatus {
                mode: "missing".to_string(),
                app_configured: false,
                app_id: None,
                app_name: None,
            }
        }
    };

    // ── Ticketing ─────────────────────────────────────────────────────────
    let ticketing_system = config.general.ticketing_system;
    let (ticketing_label, acli_ok) = match ticketing_system {
        TicketingSystem::None => ("none", false),
        TicketingSystem::GitHub => ("github", false),
        TicketingSystem::Jira => {
            let ok = check_acli_auth();
            if !ok {
                warnings.push(StructuredWarning::warning(
                    "acli_not_authenticated",
                    "Atlassian CLI (acli) is not authenticated. Jira integration is disabled until acli is logged in.",
                ));
            }
            ("jira", ok)
        }
    };
    let ticketing = TicketingStatus {
        system: ticketing_label.to_string(),
        acli_ok,
    };

    // ── Provider ──────────────────────────────────────────────────────────
    let provider = match config.agent.provider {
        AiAgentProvider::Claude => {
            let env_credential = std::env::var("CLAUDE_CODE_OAUTH_TOKEN")
                .ok()
                .map(|v| !v.trim().is_empty())
                .unwrap_or(false);
            // Phase 1: prefer the [agent.providers.claude].base_url config
            // value; fall back to the ANTHROPIC_BASE_URL env var the way
            // setup scripts used to surface it.
            let custom_base_url = {
                let cfg_url = config.agent.providers.claude.base_url.trim();
                if !cfg_url.is_empty() {
                    Some(cfg_url.to_string())
                } else {
                    std::env::var("ANTHROPIC_BASE_URL")
                        .ok()
                        .map(|v| v.trim().to_string())
                        .filter(|v| !v.is_empty())
                }
            };
            let cli_ok = env_credential || auth_cmd_ok("claude", &["auth", "status"]);
            if !cli_ok {
                warnings.push(StructuredWarning::critical(
                    "claude_not_authenticated",
                    "Claude Code is not authenticated and no CLAUDE_CODE_OAUTH_TOKEN env var is set.",
                ));
            }
            ProviderStatus {
                selected: "claude".to_string(),
                deployment_default_credential_present: env_credential,
                headless_capable: cli_ok,
                custom_base_url,
            }
        }
        AiAgentProvider::Cursor => {
            let env_credential = std::env::var("CURSOR_API_KEY")
                .ok()
                .map(|v| !v.trim().is_empty())
                .unwrap_or(false);
            let cli = config.agent.effective_cursor_cli().trim();
            if cli.is_empty() {
                warnings.push(StructuredWarning::critical(
                    "cursor_cli_missing",
                    "[agent.providers.cursor].cli must be set when provider is \"cursor\".",
                ));
                ProviderStatus {
                    selected: "cursor".to_string(),
                    deployment_default_credential_present: env_credential,
                    headless_capable: env_credential,
                    custom_base_url: None,
                }
            } else {
                let on_disk = cursor_agent_auth_likely_on_disk();
                let headless = env_credential || on_disk;
                if !headless {
                    warnings.push(StructuredWarning::critical(
                        "cursor_not_authenticated",
                        "Cursor is not authenticated and CURSOR_API_KEY is not set.",
                    ));
                }
                ProviderStatus {
                    selected: "cursor".to_string(),
                    deployment_default_credential_present: env_credential,
                    headless_capable: headless,
                    custom_base_url: None,
                }
            }
        }
        // Phase 4: Codex and OpenCode have full adapters. The headless_capable
        // flag mirrors claude/cursor — true when there's a CLI binary on PATH
        // in the worker image (both are baked into the maestro image today).
        AiAgentProvider::Codex => ProviderStatus {
            selected: "codex".to_string(),
            deployment_default_credential_present: !std::env::var("OPENAI_API_KEY")
                .unwrap_or_default()
                .is_empty(),
            headless_capable: true,
            custom_base_url: {
                let url = config.agent.providers.codex.base_url.trim();
                if url.is_empty() {
                    None
                } else {
                    Some(url.to_string())
                }
            },
        },
        AiAgentProvider::OpenCode => ProviderStatus {
            selected: "opencode".to_string(),
            deployment_default_credential_present: !std::env::var("ANTHROPIC_API_KEY")
                .unwrap_or_default()
                .is_empty(),
            headless_capable: true,
            custom_base_url: {
                let url = config.agent.providers.opencode.base_url.trim();
                if url.is_empty() {
                    None
                } else {
                    Some(url.to_string())
                }
            },
        },
    };

    SystemStatus {
        // We got here, so the config is loaded and parseable.
        config_toml_ok: true,
        github,
        provider,
        ticketing,
        // Phase 0 ships pre-DB; populated by the caller when the DB is
        // available (see `maestro-cli` `run_server`). Default `false` so the
        // CLI's `preflight` subcommand gives a sensible standalone answer.
        per_user_required: false,
        warnings,
    }
}

/// Check whether acli (Atlassian CLI) is currently authenticated.
/// This is a standalone helper so callers (e.g. the server startup) can probe acli without
/// running the full preflight sequence.
pub fn check_acli_auth() -> bool {
    auth_cmd_ok("acli", &["jira", "auth", "status"])
}

/// Verify required CLIs for the configured AI provider.
///
/// **Deprecated** in favour of [`collect_system_status`] (Phase 0 soft-fail
/// model). Kept for one release for any external caller — internal callers
/// should switch to `collect_system_status`, treat all results as informational,
/// and let the dashboard render the degraded-mode banner.
///
/// Behaviour: collects the full [`SystemStatus`], then returns `Err` iff a
/// `severity = "critical"` warning exists. `acli` failures remain soft-fail
/// (warning, not critical).
#[deprecated(note = "use collect_system_status")]
pub fn preflight(config: &Config) -> Result<PreflightResult> {
    let status = collect_system_status(config);

    // Echo the structured warnings to stderr so existing callers still see the
    // diagnostic text they used to get from inline `eprintln!`s.
    for w in &status.warnings {
        eprintln!(
            "[maestro preflight] {sev}: {code} — {msg}",
            sev = w.severity,
            code = w.code,
            msg = w.message
        );
    }

    if status.has_critical() {
        let msg = status
            .warnings
            .iter()
            .filter(|w| w.severity == "critical")
            .map(|w| format!("{}: {}", w.code, w.message))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(MaestroError::Config(msg));
    }

    eprintln!("[maestro preflight] OK.");
    Ok(PreflightResult {
        acli_ok: status.ticketing.acli_ok,
    })
}

#[cfg(test)]
mod system_status_tests {
    //! Phase 0 unit tests — verify `collect_system_status` never returns Err
    //! and emits the right structured warnings for misconfigured providers.
    //!
    //! These tests manipulate process-global env vars (`HOME`,
    //! `CLAUDE_CODE_OAUTH_TOKEN`, `CURSOR_API_KEY`, …). They run in a
    //! `tokio::sync::Mutex`-style serial-test fixture to avoid stomping on
    //! each other; we use a plain `Mutex<()>` because `serial_test` is not a
    //! workspace dep.
    use super::*;
    use crate::config::{AgentConfig, AiAgentProvider, Config, TicketingSystem};
    use std::sync::Mutex;
    use tempfile::tempdir;

    /// Serialises every test in this module so concurrent runs do not race on
    /// the process env. `std::sync::Mutex` is fine because the locked region
    /// is purely synchronous (no `.await`).
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Build a Config with the given provider, defaulting every other knob.
    fn config_with_provider(provider: AiAgentProvider) -> Config {
        let mut cfg = Config {
            agent: AgentConfig {
                provider,
                ..AgentConfig::default()
            },
            ..Config::default()
        };
        cfg.general.ticketing_system = TicketingSystem::None;
        cfg
    }

    /// Set HOME to an empty temp dir so the cursor on-disk auth probe sees
    /// no credentials, and clear every provider env var. Returns the temp
    /// dir handle so the caller keeps it alive for the duration of the test.
    fn isolate_env() -> tempfile::TempDir {
        let d = tempdir().unwrap();
        // SAFETY: tests in this module are serialised via ENV_LOCK.
        unsafe {
            std::env::set_var("HOME", d.path());
            std::env::remove_var("CLAUDE_CODE_OAUTH_TOKEN");
            std::env::remove_var("CURSOR_API_KEY");
            std::env::remove_var("ANTHROPIC_BASE_URL");
            std::env::remove_var("CURSOR_CONFIG_DIR");
            std::env::remove_var("XDG_CONFIG_HOME");
        }
        d
    }

    #[test]
    fn claude_misconfigured_produces_critical_warning_no_err() {
        let _g = ENV_LOCK.lock().unwrap();
        let _home = isolate_env();
        let cfg = config_with_provider(AiAgentProvider::Claude);

        let status = collect_system_status(&cfg);

        // No Err, structured output.
        assert!(status.config_toml_ok);
        assert_eq!(status.provider.selected, "claude");
        assert!(!status.provider.deployment_default_credential_present);
        assert!(!status.provider.headless_capable);
        assert!(
            status
                .warnings
                .iter()
                .any(|w| w.code == "claude_not_authenticated" && w.severity == "critical"),
            "expected critical claude_not_authenticated warning; got {:?}",
            status.warnings
        );
        assert!(status.has_critical());
    }

    #[test]
    fn cursor_misconfigured_produces_critical_warning_no_err() {
        let _g = ENV_LOCK.lock().unwrap();
        let _home = isolate_env();
        let cfg = config_with_provider(AiAgentProvider::Cursor);

        let status = collect_system_status(&cfg);

        assert!(status.config_toml_ok);
        assert_eq!(status.provider.selected, "cursor");
        assert!(!status.provider.deployment_default_credential_present);
        assert!(!status.provider.headless_capable);
        assert!(
            status
                .warnings
                .iter()
                .any(|w| w.code == "cursor_not_authenticated" && w.severity == "critical"),
            "expected critical cursor_not_authenticated warning; got {:?}",
            status.warnings
        );
        assert!(status.has_critical());
    }

    #[test]
    fn claude_with_env_token_is_headless_capable() {
        let _g = ENV_LOCK.lock().unwrap();
        let _home = isolate_env();
        // SAFETY: serialised via ENV_LOCK.
        unsafe {
            std::env::set_var("CLAUDE_CODE_OAUTH_TOKEN", "sk-ant-test");
            std::env::set_var("ANTHROPIC_BASE_URL", "https://proxy.example.com");
        }
        let cfg = config_with_provider(AiAgentProvider::Claude);

        let status = collect_system_status(&cfg);

        assert_eq!(status.provider.selected, "claude");
        assert!(status.provider.deployment_default_credential_present);
        assert!(status.provider.headless_capable);
        assert_eq!(
            status.provider.custom_base_url.as_deref(),
            Some("https://proxy.example.com")
        );
        // No provider-related critical warnings.
        assert!(
            !status
                .warnings
                .iter()
                .any(|w| w.code == "claude_not_authenticated"),
            "claude warning should be absent when token is set; got {:?}",
            status.warnings
        );

        // Clean up env so sibling tests are not polluted.
        // SAFETY: serialised via ENV_LOCK.
        unsafe {
            std::env::remove_var("CLAUDE_CODE_OAUTH_TOKEN");
            std::env::remove_var("ANTHROPIC_BASE_URL");
        }
    }

    #[test]
    fn cursor_with_env_key_is_headless_capable() {
        let _g = ENV_LOCK.lock().unwrap();
        let _home = isolate_env();
        // SAFETY: serialised via ENV_LOCK.
        unsafe {
            std::env::set_var("CURSOR_API_KEY", "ck_test_token");
        }
        let cfg = config_with_provider(AiAgentProvider::Cursor);

        let status = collect_system_status(&cfg);

        assert_eq!(status.provider.selected, "cursor");
        assert!(status.provider.deployment_default_credential_present);
        assert!(status.provider.headless_capable);
        assert!(
            !status
                .warnings
                .iter()
                .any(|w| w.code == "cursor_not_authenticated"),
            "cursor warning should be absent when key is set; got {:?}",
            status.warnings
        );

        // SAFETY: serialised via ENV_LOCK.
        unsafe {
            std::env::remove_var("CURSOR_API_KEY");
        }
    }

    /// T-BOOT-003 (P0): when every check is failing, `collect_system_status`
    /// must return a `SystemStatus` (never `Err`), and every warning it emits
    /// must have a non-empty structured `code` — the UI relies on `code` for
    /// localised copy and setup links, free-form `message` text alone is not
    /// sufficient.
    #[test]
    fn collect_system_status_returns_struct_with_codes_when_everything_broken() {
        let _g = ENV_LOCK.lock().unwrap();
        let _home = isolate_env();
        let mut cfg = config_with_provider(AiAgentProvider::Claude);
        // Force the ticketing branch to flag too.
        cfg.general.ticketing_system = TicketingSystem::Jira;

        let status = collect_system_status(&cfg);

        // 1) Never an Err — the function signature already guarantees this,
        //    but the assertion below documents the contract.
        assert!(!status.warnings.is_empty(), "expected ≥1 warning");
        // 2) Every warning is structured: non-empty `code` and a known severity.
        for w in &status.warnings {
            assert!(!w.code.is_empty(), "warning {:?} has empty code", w);
            assert!(
                matches!(w.severity.as_str(), "critical" | "warning" | "info"),
                "warning {:?} has unknown severity",
                w
            );
        }
        // 3) The provider blocker must be enumerated as a critical warning.
        assert!(
            status
                .warnings
                .iter()
                .any(|w| w.code == "claude_not_authenticated" && w.severity == "critical"),
            "expected critical claude_not_authenticated; got {:?}",
            status.warnings
        );
        // 4) `has_critical()` reflects the warning set.
        assert!(status.has_critical());
    }

    #[test]
    fn ticketing_jira_without_acli_emits_warning_not_critical() {
        let _g = ENV_LOCK.lock().unwrap();
        let _home = isolate_env();
        // SAFETY: serialised via ENV_LOCK.
        unsafe {
            // Make claude headless so the provider check stays clean.
            std::env::set_var("CLAUDE_CODE_OAUTH_TOKEN", "sk-ant-test");
        }
        let mut cfg = config_with_provider(AiAgentProvider::Claude);
        cfg.general.ticketing_system = TicketingSystem::Jira;

        let status = collect_system_status(&cfg);

        assert_eq!(status.ticketing.system, "jira");
        // acli probe is unlikely to succeed in CI — accept either branch, but
        // when it fails the warning must be `warning` (not critical).
        if !status.ticketing.acli_ok {
            let acli_w = status
                .warnings
                .iter()
                .find(|w| w.code == "acli_not_authenticated")
                .expect("expected acli_not_authenticated warning");
            assert_eq!(
                acli_w.severity, "warning",
                "acli failure must be a soft-fail (warning), not critical"
            );
        }

        // SAFETY: serialised via ENV_LOCK.
        unsafe {
            std::env::remove_var("CLAUDE_CODE_OAUTH_TOKEN");
        }
    }

    // ── Task #37 (Phase 2c): config-dir writability probe ───────────────

    /// Happy path: a freshly-created tempdir is writable → no warning.
    #[test]
    fn check_config_dir_writable_returns_none_for_writable_dir() {
        let dir = tempdir().unwrap();
        let cfg_path = dir.path().join("config.toml");
        let result = check_config_dir_writable(&cfg_path);
        assert!(result.is_none(), "writable dir must not emit warning");
    }

    /// Parent dir missing → no warning (avoids double-flagging the
    /// already-covered `config_missing` case).
    #[test]
    fn check_config_dir_writable_returns_none_when_parent_missing() {
        let cfg_path = std::path::Path::new(
            "/nonexistent/maestro-task-37/parent/config.toml",
        );
        let result = check_config_dir_writable(cfg_path);
        assert!(
            result.is_none(),
            "missing parent dir must not emit warning (config_missing covers it)"
        );
    }

    /// Read-only directory → emits `config_dir_not_writable` warning.
    /// Skipped when running as root (no permission denial possible).
    #[cfg(unix)]
    #[test]
    fn check_config_dir_writable_emits_warning_for_readonly_dir() {
        use std::os::unix::fs::PermissionsExt;
        // Running as root means chmod 0500 doesn't actually deny writes —
        // CAP_DAC_OVERRIDE bypasses POSIX perms. Skip the assertion in
        // that case; the test still proves the function compiles and
        // returns None (root effectively can always write).
        let is_root = unsafe { libc::geteuid() } == 0;
        let dir = tempdir().unwrap();
        let cfg_path = dir.path().join("config.toml");

        // Make the dir read-only.
        let mut perms = std::fs::metadata(dir.path()).unwrap().permissions();
        perms.set_mode(0o500);
        std::fs::set_permissions(dir.path(), perms).unwrap();

        let result = check_config_dir_writable(&cfg_path);

        // Restore perms BEFORE asserting so tempdir cleanup succeeds.
        let mut restore = std::fs::metadata(dir.path()).unwrap().permissions();
        restore.set_mode(0o700);
        std::fs::set_permissions(dir.path(), restore).unwrap();

        if !is_root {
            let w = result.expect("read-only dir must emit warning");
            assert_eq!(w.code, "config_dir_not_writable");
            assert_eq!(w.severity, "critical");
            assert!(
                w.message.contains(dir.path().to_string_lossy().as_ref()),
                "warning message must include the failing path; got: {}",
                w.message
            );
        }
    }
}
