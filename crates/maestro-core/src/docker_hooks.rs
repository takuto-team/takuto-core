// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Config-driven shell hooks for Docker image build and container startup.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value as JsonValue;

use crate::config::{AiAgentProvider, Config, TicketingSystem};
use crate::error::{MaestroError, Result};

fn preflight_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/home/maestro"))
}

/// Cursor CLI stores browser-login state under `CURSOR_CONFIG_DIR` (default `~/.cursor`).
/// `agent status` often returns non-zero without a TTY even when login succeeded, and the JSON schema
/// for tokens changes between releases — so we also accept “this tree clearly has Cursor CLI data”.
fn cursor_agent_auth_likely_on_disk() -> bool {
    let config_dir = std::env::var_os("CURSOR_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| preflight_home().join(".cursor"));

    let mut paths = vec![config_dir.join("cli-config.json")];
    if let Some(x) = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from) {
        paths.push(x.join("cursor/cli-config.json"));
    } else {
        paths.push(preflight_home().join(".config/cursor/cli-config.json"));
    }

    for p in &paths {
        if json_config_suggests_auth(p) {
            return true;
        }
    }

    // Any other *.json next to cli-config (Cursor versions may rename or split fields)
    if let Ok(rd) = std::fs::read_dir(&config_dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_file()
                && p.extension().and_then(|s| s.to_str()) == Some("json")
                && !paths.iter().any(|known| known == &p)
                && json_config_suggests_auth(&p)
            {
                return true;
            }
        }
    }

    // Browser login may store state in nested dirs / non-JSON files; `agent status` is unreliable headless.
    let xdg_cursor = preflight_home().join(".config/Cursor");
    let xdg_cursor_lower = preflight_home().join(".config/cursor");
    cursor_data_tree_looks_populated(&config_dir)
        || cursor_data_tree_looks_populated(&xdg_cursor)
        || cursor_data_tree_looks_populated(&xdg_cursor_lower)
}

/// True if the directory contains a small amount of non-trivial file data typical after `agent login` / CLI use.
fn cursor_data_tree_looks_populated(root: &Path) -> bool {
    if !root.is_dir() {
        return false;
    }

    fn walk(dir: &Path, depth: u8) -> bool {
        if depth > 10 {
            return false;
        }
        let Ok(rd) = std::fs::read_dir(dir) else {
            return false;
        };
        for ent in rd.flatten() {
            let p = ent.path();
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let low = name.to_lowercase();
            if low == ".ds_store" || low.contains("readme") {
                continue;
            }
            if p.is_dir() {
                if walk(&p, depth + 1) {
                    return true;
                }
            } else if let Ok(meta) = p.metadata() {
                if !meta.is_file() {
                    continue;
                }
                let len = meta.len();
                if len < 16 {
                    continue;
                }
                if low.ends_with(".log") && len < 256 {
                    continue;
                }
                // SQLite / VS Code style state DBs
                if low.ends_with(".vscdb") || low.ends_with(".db") {
                    return true;
                }
                if low.ends_with(".json") {
                    if let Ok(raw) = std::fs::read_to_string(&p)
                        && let Ok(v) = serde_json::from_str::<JsonValue>(&raw)
                    {
                        if json_value_has_auth_fields(&v) {
                            return true;
                        }
                        if v.as_object().is_some_and(|m| m.len() >= 2 && len >= 32) {
                            return true;
                        }
                    }
                    continue;
                }
                // Any other non-trivial file (e.g. binary token blob)
                if len >= 48 {
                    return true;
                }
            }
        }
        false
    }

    walk(root, 0)
}

fn json_config_suggests_auth(path: &Path) -> bool {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(v) = serde_json::from_str::<JsonValue>(&raw) else {
        return false;
    };
    json_value_has_auth_fields(&v)
}

fn json_value_has_auth_fields(v: &JsonValue) -> bool {
    match v {
        JsonValue::Object(map) => {
            // Cursor may store opaque session strings without "token" in the key name.
            for val in map.values() {
                if val.as_str().is_some_and(|s| s.len() >= 64) {
                    return true;
                }
            }
            for (k, val) in map {
                let kl = k.to_lowercase();
                if (kl.contains("token") || kl.ends_with("apikey") || kl == "api_key")
                    && val.as_str().is_some_and(|s| !s.trim().is_empty())
                {
                    return true;
                }
            }
            map.values().any(json_value_has_auth_fields)
        }
        JsonValue::Array(items) => items.iter().any(json_value_has_auth_fields),
        JsonValue::String(s) if s.len() >= 64 => true,
        _ => false,
    }
}

#[cfg(unix)]
fn configure_auth_command_unix(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        cmd.pre_exec(|| {
            libc::setpgid(0, 0);
            Ok(())
        });
    }
}

#[cfg(not(unix))]
fn configure_auth_command_unix(_cmd: &mut Command) {}

#[cfg(unix)]
fn kill_process_group_best_effort(child: &mut std::process::Child) {
    let pid = child.id();
    if pid > 0 {
        unsafe {
            let _ = libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(not(unix))]
fn kill_process_group_best_effort(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Run each non-empty command with `bash -c` in `cwd`, inheriting stdio (for logs during build/up).
/// Debian `sh` is often **dash**, which does not support `set -o pipefail` and other bash-isms used in hooks.
pub fn run_hook_commands(commands: &[String], cwd: &Path, label: &str) -> Result<()> {
    if commands.is_empty() {
        return Ok(());
    }

    let _ = std::fs::create_dir_all(cwd);

    let total = commands.iter().filter(|c| !c.trim().is_empty()).count();
    let mut n = 0usize;
    for cmd_line in commands {
        if cmd_line.trim().is_empty() {
            continue;
        }
        n += 1;
        let preview: String = cmd_line.chars().take(100).collect();
        let dots = if cmd_line.len() > 100 { "…" } else { "" };
        eprintln!(
            "[maestro docker-hooks:{label}] ({n}/{total}) cwd={} script={}{} ({} bytes)",
            cwd.display(),
            preview,
            dots,
            cmd_line.len()
        );
        eprintln!("[maestro docker-hooks:{label}] ({n}/{total}) running…");
        // Prefer MAESTRO_HOME so hooks writing "$HOME/.claude" land on the named volume, not on ephemeral
        // paths like /.claude when HOME is missing under Podman/rootless.
        let home = std::env::var("MAESTRO_HOME")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_else(|_| "/home/maestro".to_string());
        let cursor_dir = std::env::var("CURSOR_CONFIG_DIR")
            .unwrap_or_else(|_| format!("{}/.cursor", home.trim_end_matches('/')));
        let mut cmd = Command::new("/bin/bash");
        cmd.arg("-c")
            .arg(cmd_line)
            .current_dir(cwd)
            .env("HOME", &home)
            .env("MAESTRO_HOME", &home)
            .env("CURSOR_CONFIG_DIR", &cursor_dir)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        let status = cmd
            .status()
            .map_err(|e| MaestroError::Config(format!("failed to spawn {label} hook {n}: {e}")))?;
        if !status.success() {
            return Err(MaestroError::Config(format!(
                "{label} hook command {n} failed with status {status}"
            )));
        }
        eprintln!("[maestro docker-hooks:{label}] ({n}/{total}) finished successfully.");
    }
    Ok(())
}

/// Run an auth probe with a wall-clock timeout so `docker compose up` cannot hang forever
/// (e.g. Cursor `agent status` waiting on network without a client-side deadline).
fn auth_cmd_ok(program: &str, args: &[&str]) -> bool {
    let timeout = if args == ["status"] {
        Duration::from_secs(45)
    } else {
        Duration::from_secs(30)
    };

    let mut cmd = Command::new(program);
    cmd.args(args).stdout(Stdio::null()).stderr(Stdio::null());
    configure_auth_command_unix(&mut cmd);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => return false,
    };

    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => {}
            Err(_) => {
                kill_process_group_best_effort(&mut child);
                return false;
            }
        }
        if start.elapsed() >= timeout {
            kill_process_group_best_effort(&mut child);
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Result of the preflight check. Hard failures (gh, provider) are still returned as `Err`.
/// Soft-fail items (acli) are captured here.
pub struct PreflightResult {
    /// `true` when `acli jira auth status` succeeded.
    pub acli_ok: bool,
}

// ---------------------------------------------------------------------------
// Phase 0 — Structured SystemStatus (boot soft-fail)
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
    /// otherwise. Phase 2 will add the per-user PAT layer (FR-4.2).
    pub mode: String,
    pub app_configured: bool,
    pub app_id: Option<u64>,
    pub app_name: Option<String>,
}

/// Provider integration state for the active AI agent (`[agent] provider`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProviderStatus {
    /// `"claude" | "cursor" | "codex" | "opencode" | "none"`. Only `claude` and
    /// `cursor` are wired in v0; the other values are reserved for Phase 4.
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
    fn critical(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            severity: "critical".to_string(),
            message: message.into(),
        }
    }

    fn warning(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            severity: "warning".to_string(),
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
        // Phase 1 (04_architecture.md §0 D1, §12 Phase 4): Codex and OpenCode
        // are config-only placeholders in v1. The adapter wiring lands in
        // Phase 4. Until then, selecting one of these providers surfaces a
        // critical warning so the dashboard renders "Coming in Phase 4" copy
        // and the runtime refuses to spawn sessions.
        AiAgentProvider::Codex => {
            warnings.push(StructuredWarning::critical(
                "provider_not_implemented",
                "Provider \"codex\" is configured but the adapter is not yet wired (Phase 4). Workflows will fail until the adapter ships.",
            ));
            ProviderStatus {
                selected: "codex".to_string(),
                deployment_default_credential_present: false,
                headless_capable: false,
                custom_base_url: {
                    let url = config.agent.providers.codex.base_url.trim();
                    if url.is_empty() {
                        None
                    } else {
                        Some(url.to_string())
                    }
                },
            }
        }
        AiAgentProvider::OpenCode => {
            warnings.push(StructuredWarning::critical(
                "provider_not_implemented",
                "Provider \"opencode\" is configured but the adapter is not yet wired (Phase 4). Workflows will fail until the adapter ships.",
            ));
            ProviderStatus {
                selected: "opencode".to_string(),
                deployment_default_credential_present: false,
                headless_capable: false,
                custom_base_url: {
                    let url = config.agent.providers.opencode.base_url.trim();
                    if url.is_empty() {
                        None
                    } else {
                        Some(url.to_string())
                    }
                },
            }
        }
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

/// Try to recover from a failed `gh auth status` by switching to a user whose oauth token starts
/// with `gho_` (personal access token — does not expire). This handles the case where a GitHub App
/// installation token (`ghs_`) was set as the active user and has since expired.
///
/// Parses `~/.config/gh/hosts.yml` for the `github.com` host, finds any user with a `gho_` token,
/// and runs `gh auth switch --user <name> --hostname github.com`. Returns `true` if we switched and
/// `gh auth status` now passes.
// TODO: This uses a fragile line-based YAML parser with hardcoded indent levels (4/8/12 spaces)
// matching the current `gh` CLI output format. Consider using a YAML library if gh changes format.
fn gh_auth_recover_expired_token() -> bool {
    let hosts_path = preflight_home().join(".config/gh/hosts.yml");
    let content = match std::fs::read_to_string(&hosts_path) {
        Ok(c) => c,
        Err(_) => return false,
    };

    // Minimal line-based parse — avoids a YAML dependency.
    // Expected structure (4-space indented YAML written by the gh CLI):
    //   github.com:
    //       users:
    //           morphet81:
    //               oauth_token: gho_...
    //           sous-coder[bot]:
    //               oauth_token: ghs_...
    let mut in_github_com = false;
    let mut in_users = false;
    let mut current_user: Option<String> = None;
    let mut personal_token_users: Vec<String> = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let indent = line.len() - line.trim_start().len();

        if !in_github_com {
            if trimmed == "github.com:" {
                in_github_com = true;
            }
            continue;
        }

        // A zero-indent non-comment line means we left the github.com block.
        if indent == 0 && !trimmed.starts_with('#') {
            break;
        }

        if trimmed == "users:" {
            in_users = true;
            current_user = None;
            continue;
        }

        if in_users {
            // A line at indent=4 that isn't "users:" signals we left the users block.
            if indent <= 4 && trimmed != "users:" {
                in_users = false;
                current_user = None;
                continue;
            }
            // Username entries sit at indent=8 and end with ':'
            if indent == 8 && trimmed.ends_with(':') {
                current_user = Some(trimmed.trim_end_matches(':').to_string());
                continue;
            }
            // Token lines are at indent=12
            if indent >= 12
                && let Some(ref user) = current_user
                && let Some(token) = trimmed.strip_prefix("oauth_token:")
            {
                let tok = token.trim();
                if tok.starts_with("gho_") {
                    personal_token_users.push(user.clone());
                }
            }
        }
    }

    for user in personal_token_users {
        let switched = Command::new("gh")
            .args([
                "auth",
                "switch",
                "--user",
                &user,
                "--hostname",
                "github.com",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if switched && auth_cmd_ok("gh", &["auth", "status"]) {
            eprintln!(
                "[maestro preflight] Auto-switched active gh user to '{user}' \
                 (previous token was expired or invalid — common with GitHub App installation tokens)."
            );
            return true;
        }
    }

    false
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
mod cursor_preflight_tests {
    use std::io::Write;

    use super::*;
    use tempfile::tempdir;

    #[test]
    fn detects_opaque_session_string_in_cli_config() {
        let d = tempdir().unwrap();
        let p = d.path().join("cli-config.json");
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(
            br#"{"session":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,
        )
        .unwrap();
        assert!(json_config_suggests_auth(&p));
    }

    #[test]
    fn tree_populated_finds_nested_vscdb() {
        let d = tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("User/globalStorage")).unwrap();
        std::fs::write(d.path().join("User/globalStorage/state.vscdb"), [0u8; 64]).unwrap();
        assert!(cursor_data_tree_looks_populated(d.path()));
    }
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
}
