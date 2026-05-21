// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{MaestroError, Result};

mod agent;
mod general;
mod git;
mod template;

pub use agent::{
    AgentConfig, AgentProviderConfig, AgentProvidersConfig, AgentStepConfig, AiAgentProvider,
    CodexProviderConfig, CursorProviderConfig, DENIED_EXTRA_ARG_FLAGS, SkillRef, StepAvailability,
    cursor_model_for_cli, validate_extra_args,
};
pub use general::{
    DevConfig, DockerConfig, GeneralConfig, ProvisioningConfig, TicketingSystem,
};
pub use git::{GitConfig, GitHubAppConfig};
pub use template::{interpolate_agent_prompt, interpolate_command_template};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub general: GeneralConfig,
    #[serde(default)]
    pub jira: JiraConfig,
    #[serde(default)]
    pub git: GitConfig,
    #[serde(default)]
    pub github: GitHubAppConfig,
    #[serde(default)]
    pub web: WebConfig,
    #[serde(default)]
    pub agent: AgentConfig,
    #[serde(default)]
    pub docker: DockerConfig,
    #[serde(default)]
    pub network: NetworkConfig,
    #[serde(default)]
    pub editor: EditorConfig,
    #[serde(default)]
    pub terminal: TerminalConfig,
    /// Dev-only knobs. Off by default in production. Never read inside any code path
    /// that runs against real users without an explicit `[dev]` opt-in.
    #[serde(default)]
    pub dev: DevConfig,
    /// Task #47: admin-supplied tool installs that run at maestro startup
    /// and populate the shared `maestro-tools` volume. See
    /// [`ProvisioningConfig`].
    #[serde(default)]
    pub provisioning: ProvisioningConfig,
}

impl Config {
    /// Task #47: canonical sha256 of the `[provisioning].install_commands`
    /// list. The bytes hashed are the JSON-encoded array of commands
    /// (preserves order — the admin's order matters because a later
    /// command can depend on artifacts from an earlier one). Whitespace
    /// inside a command is part of its identity; whitespace between
    /// elements is not (it's encoded as a single JSON array). Stable
    /// across runs, machines, and serde-toml versions.
    pub fn provisioning_sha(&self) -> String {
        use sha2::{Digest, Sha256};
        let json = serde_json::to_string(&self.provisioning.install_commands)
            .unwrap_or_else(|_| "[]".to_string());
        let mut h = Sha256::new();
        h.update(json.as_bytes());
        format!("{:x}", h.finalize())
    }
}

/// Browser-based VS Code editor launched from the dashboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditorConfig {
    /// Application ports to expose in the editor container (e.g. `[3000, 5173]`).
    /// Each port is mapped to a host port from the DinD range (9100–9200).
    #[serde(default)]
    pub ports: Vec<u16>,
    /// Number of extra ports pre-allocated for dynamic forwarding (auto-detected dev servers).
    /// Set to `0` to disable dynamic port forwarding. Default: `10`.
    #[serde(default = "default_dynamic_ports")]
    pub dynamic_ports: usize,
    /// VS Code color theme (e.g. `"One Dark Pro"`, `"GitHub Dark"`).
    #[serde(default)]
    pub theme: String,
    /// Extension marketplace IDs to pre-install (e.g. `["esbenp.prettier-vscode"]`).
    #[serde(default)]
    pub extensions: Vec<String>,
    /// Arbitrary VS Code settings written to `settings.json`.
    /// Keys are VS Code setting paths (e.g. `"editor.fontSize" = 14`).
    #[serde(default)]
    pub settings: std::collections::HashMap<String, toml::Value>,
}

fn default_dynamic_ports() -> usize {
    10
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            ports: Vec::new(),
            dynamic_ports: default_dynamic_ports(),
            theme: String::new(),
            extensions: Vec::new(),
            settings: std::collections::HashMap::new(),
        }
    }
}

/// Web terminal (ttyd) customization launched from the dashboard.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TerminalConfig {
    /// Shell commands run once per editor container lifetime, before the first ttyd session.
    /// Use this for expensive one-time setup (apt installs, tool configuration).
    /// Commands are executed with `/etc/maestro/env` sourced so API tokens are available.
    /// Guarded by `/tmp/.maestro-terminal-setup-done` — won't re-run on the same container.
    #[serde(default)]
    pub setup_commands: Vec<String>,
    /// Shell commands run every time a fresh editor container is created.
    /// Use for tools that should be refreshed on each editor open, e.g.:
    ///   `mise use -g ruby@3.3` — installs on first open, verifies on subsequent opens.
    /// Installs via mise persist in the shared mise volume so only the first run is slow.
    /// `/etc/maestro/env` is sourced before each command.
    #[serde(default)]
    pub startup_commands: Vec<String>,
    /// Default git editor installed and configured inside every editor container.
    /// Set to a package name available via apt (e.g. `"nano"`, `"vim"`, `"micro"`).
    /// When set, the package is installed and `git config --global core.editor` is
    /// updated for the `maestro` user. Empty string (default) leaves git's default.
    #[serde(default)]
    pub git_editor: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NetworkConfig {
    #[serde(default)]
    pub extra_egress_hosts: Vec<String>,
    #[serde(default)]
    pub allow_all_https: bool,
}

/// How linked Jira issues are included in `{ticket_context}` for agent prompts.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum LinkedItemsPromptMode {
    /// Key, summary, status, link type, and description (subject to byte caps).
    #[default]
    Full,
    /// Key, summary, status, and link type only (descriptions omitted).
    SummaryOnly,
    /// Linked issues are not included in the context string.
    Omit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JiraConfig {
    #[serde(default)]
    pub project_keys: Vec<String>,
    #[serde(default = "default_item_types")]
    pub item_types: Vec<String>,
    #[serde(default)]
    pub jql_filter: String,
    #[serde(default)]
    pub site: String,
    #[serde(default)]
    pub email: String,
    /// Status name for **Mark as Done** (Jira transition target). Must match your workflow.
    #[serde(default = "default_jira_done_status")]
    pub done_status: String,
    /// How linked issues appear in agent prompts (`{ticket_context}`).
    #[serde(default)]
    pub linked_items_in_prompt: LinkedItemsPromptMode,
    /// Max UTF-8 bytes for the primary ticket description in prompts (`0` = unlimited).
    #[serde(default)]
    pub ticket_context_max_description_bytes: usize,
    /// Max UTF-8 bytes per linked issue description when mode is `full` (`0` = unlimited).
    #[serde(default)]
    pub linked_issue_description_max_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    /// When **both** `dashboard_username` and `dashboard_password` are set, the dashboard API and WebSocket require a signed session cookie (see `POST /api/auth/login`). Password is never returned by `GET /api/config`.
    #[serde(default)]
    pub dashboard_username: String,
    #[serde(default)]
    pub dashboard_password: String,
    /// Allowed CORS origins (e.g. `["http://localhost:8080", "https://maestro.example.com"]`).
    /// When empty (default), auto-computed from `host` and `port`.
    /// Startup-only — not patchable via `PUT /api/config`.
    #[serde(default)]
    pub cors_origins: Vec<String>,
    /// When set, controls the `Secure` flag on session cookies.
    /// `None` (default) auto-detects: `true` if any `cors_origins` entry is `https://…`
    /// or the inbound request carries `X-Forwarded-Proto: https`.
    #[serde(default)]
    pub cookie_secure: Option<bool>,
    /// Plan-02 AC-5: whether a successful login deletes prior sessions for the
    /// same user. Defaults to `true` (security-first). Set to `false` if your
    /// users routinely log in from multiple clients concurrently and the UX
    /// cost of forcing re-login on every new login outweighs the security
    /// benefit of single-session enforcement.
    #[serde(default = "default_kick_other_sessions")]
    pub kick_other_sessions_on_login: bool,
}

impl WebConfig {
    /// `true` when username (trimmed) and password are both non-empty.
    pub fn dashboard_auth_enabled(&self) -> bool {
        !self.dashboard_username.trim().is_empty() && !self.dashboard_password.is_empty()
    }

    /// Normalize `cors_origins` in place: strip default ports (:80 for http, :443 for https).
    /// Invalid entries are kept unchanged so that `Config::validate()` can report them as errors.
    /// Call this before `Config::validate()` so validation sees the canonical form.
    pub fn normalize_cors_origins(&mut self) {
        self.cors_origins = self
            .cors_origins
            .iter()
            .map(|o| validate_cors_origin(o).unwrap_or_else(|_| o.clone()))
            .collect();
    }

    /// Return the effective CORS origins: the explicit list if non-empty,
    /// otherwise a sensible default derived from `host` and `port`.
    pub fn resolved_cors_origins(&self) -> Vec<String> {
        if !self.cors_origins.is_empty() {
            return self.cors_origins.clone();
        }
        // Auto-compute: when binding to a wildcard or loopback address, the dashboard
        // is reachable via multiple hostnames (localhost, 127.0.0.1, 0.0.0.0, etc.).
        // Include all common variants so the CORS check passes regardless of which
        // hostname the operator typed in the browser address bar.
        let host = self.host.trim();
        let is_wildcard = host == "0.0.0.0" || host == "[::]";
        let is_loopback = host == "127.0.0.1" || host == "::1";
        if is_wildcard {
            vec![
                format!("http://localhost:{}", self.port),
                format!("http://127.0.0.1:{}", self.port),
                format!("http://0.0.0.0:{}", self.port),
            ]
        } else if is_loopback {
            // IPv6 addresses in URLs must be bracketed (RFC 2732).
            let host_part = if host.contains(':') {
                format!("[{}]", host)
            } else {
                host.to_string()
            };
            vec![
                format!("http://localhost:{}", self.port),
                format!("http://{}:{}", host_part, self.port),
            ]
        } else {
            // Bracket IPv6 literal addresses in the origin URL.
            let host_part = if host.contains(':') {
                format!("[{}]", host)
            } else {
                host.to_string()
            };
            vec![format!("http://{}:{}", host_part, self.port)]
        }
    }
}

/// Validate a single CORS origin string.
/// Must start with `http://` or `https://`, must have no path component (no `/` after the authority).
/// Normalizes default ports: strips `:80` from `http://` and `:443` from `https://`.
pub fn validate_cors_origin(origin: &str) -> std::result::Result<String, String> {
    let trimmed = origin.trim();
    if trimmed.is_empty() {
        return Err("[web] cors_origins: entry must not be empty".into());
    }

    let (scheme, authority) = if let Some(rest) = trimmed.strip_prefix("https://") {
        ("https", rest)
    } else if let Some(rest) = trimmed.strip_prefix("http://") {
        ("http", rest)
    } else {
        return Err(format!(
            "[web] cors_origins: '{trimmed}' must start with http:// or https://"
        ));
    };

    if authority.is_empty() {
        return Err(format!(
            "[web] cors_origins: '{trimmed}' has no host after scheme"
        ));
    }

    // Origins must not contain a path — no `/` in the authority portion.
    if authority.contains('/') {
        return Err(format!(
            "[web] cors_origins: '{trimmed}' must not contain a path (no '/' after the host)"
        ));
    }

    // Normalize default ports: strip :80 for http, :443 for https.
    let normalized = match scheme {
        "http" if authority.ends_with(":80") => {
            format!("http://{}", authority.strip_suffix(":80").unwrap())
        }
        "https" if authority.ends_with(":443") => {
            format!("https://{}", authority.strip_suffix(":443").unwrap())
        }
        _ => format!("{scheme}://{authority}"),
    };

    Ok(normalized)
}

// Default value functions

fn default_item_types() -> Vec<String> {
    vec!["Task".to_string(), "Bug".to_string()]
}
fn default_host() -> String {
    "0.0.0.0".to_string()
}
fn default_port() -> u16 {
    8080
}

fn default_jira_done_status() -> String {
    "Done".to_string()
}

impl Default for JiraConfig {
    fn default() -> Self {
        Self {
            project_keys: Vec::new(),
            item_types: default_item_types(),
            jql_filter: String::new(),
            site: String::new(),
            email: String::new(),
            done_status: default_jira_done_status(),
            linked_items_in_prompt: LinkedItemsPromptMode::default(),
            ticket_context_max_description_bytes: 0,
            linked_issue_description_max_bytes: 0,
        }
    }
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            dashboard_username: String::new(),
            dashboard_password: String::new(),
            cors_origins: Vec::new(),
            cookie_secure: None,
            kick_other_sessions_on_login: default_kick_other_sessions(),
        }
    }
}

fn default_kick_other_sessions() -> bool {
    true
}

pub fn resolve_config_relative_path(config_file_dir: &Path, rel: &str) -> PathBuf {
    let t = rel.trim();
    if t.is_empty() {
        return PathBuf::new();
    }
    let p = PathBuf::from(t);
    if p.is_absolute() {
        p
    } else {
        config_file_dir.join(p)
    }
}

/// Emit a startup warning if the loaded TOML still contains the legacy
/// `[commands]` table or `[[run_commands]]` array.
///
/// As of plan-09 these settings live per-user-per-workspace in the database
/// and are configured from the dashboard's Configuration → Worktree Settings
/// tab. Stale entries in `config.toml` are silently ignored at parse time;
/// this warning exists so operators notice the migration.
/// Detect stale `[commands]` / `[[run_commands]]` top-level keys in a config
/// file. Returns the warning messages that callers should emit; pure so the
/// caller can defer emission until after `tracing_subscriber` is initialised
/// (otherwise the warning is silently dropped because the global subscriber
/// is a no-op until init runs).
pub fn detect_legacy_command_keys(toml_content: &str) -> Vec<&'static str> {
    let mut warnings = Vec::new();
    let Ok(value) = toml::from_str::<toml::Value>(toml_content) else {
        return warnings;
    };
    let Some(table) = value.as_table() else {
        return warnings;
    };
    if table.contains_key("commands") {
        warnings.push(
            "config.toml: `[commands]` table is ignored — worktree init commands are now per-user. \
             Configure them in the dashboard's Configuration → Worktree Settings tab.",
        );
    }
    if table.contains_key("run_commands") {
        warnings.push(
            "config.toml: `[[run_commands]]` entries are ignored — run commands are now per-user. \
             Configure them in the dashboard's Configuration → Worktree Settings tab.",
        );
    }
    warnings
}

/// Emit any detected legacy-key warnings via `tracing::warn!`. Safe to call
/// any time tracing is initialised; on first load the caller in `maestro-cli`
/// uses [`detect_legacy_command_keys`] directly and replays the warnings
/// after tracing setup.
fn warn_legacy_command_keys(toml_content: &str) {
    for msg in detect_legacy_command_keys(toml_content) {
        tracing::warn!("{msg}");
    }
}

impl Config {
    /// Parse a `Config` from a TOML string without loading from disk.
    ///
    /// Useful for tests and scenarios where the config content is already in
    /// memory. Applies validation but **not** external workflow step files
    /// (those require a filesystem path).
    pub fn load_from_str(toml_content: &str) -> Result<Self> {
        warn_legacy_command_keys(toml_content);
        let mut config: Config = toml::from_str(toml_content)?;
        config.web.normalize_cors_origins();
        // Phase 1: migrate legacy flat [agent] keys into the new sub-tables
        // before validation so validation sees the post-migration shape.
        config.agent.migrate_legacy_flat_fields();
        config.validate()?;
        Ok(config)
    }

    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Err(MaestroError::ConfigNotFound(path.to_path_buf()));
        }

        let content = std::fs::read_to_string(path)?;
        warn_legacy_command_keys(&content);
        let mut config: Config = toml::from_str(&content)?;
        config.web.normalize_cors_origins();
        // Phase 1: migrate legacy flat [agent] keys into the new sub-tables.
        config.agent.migrate_legacy_flat_fields();
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if self.general.poll_interval_secs < 10 {
            return Err(MaestroError::Config(
                "poll_interval_secs must be at least 10".to_string(),
            ));
        }

        if self.general.max_concurrent_workflows == 0 {
            return Err(MaestroError::Config(
                "max_concurrent_workflows must be at least 1".to_string(),
            ));
        }

        if self.general.effective_max_active_workflows() < 1 {
            return Err(MaestroError::Config(
                "max_active_workflows must be at least 1 when set, or leave 0 to use max_concurrent_workflows"
                    .to_string(),
            ));
        }

        for key in &self.jira.project_keys {
            if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric()) {
                return Err(MaestroError::Config(format!(
                    "Invalid Jira project key: '{key}'. Must be non-empty uppercase alphanumeric."
                )));
            }
        }

        if self.jira.item_types.is_empty() {
            return Err(MaestroError::Config(
                "At least one Jira item type must be configured".to_string(),
            ));
        }

        if self.web.port == 0 {
            return Err(MaestroError::Config(
                "Web port must be a non-zero value".to_string(),
            ));
        }

        let du = self.web.dashboard_username.trim();
        let dp = self.web.dashboard_password.as_str();
        let has_u = !du.is_empty();
        let has_p = !dp.is_empty();
        if has_u != has_p {
            return Err(MaestroError::Config(
                "[web] set both dashboard_username and dashboard_password, or leave both empty (no dashboard auth)"
                    .to_string(),
            ));
        }
        const MAX_DASHBOARD_USER_LEN: usize = 256;
        const MAX_DASHBOARD_PASSWORD_LEN: usize = 4096;
        if du.len() > MAX_DASHBOARD_USER_LEN {
            return Err(MaestroError::Config(format!(
                "[web] dashboard_username exceeds {MAX_DASHBOARD_USER_LEN} bytes (trimmed)"
            )));
        }
        if dp.len() > MAX_DASHBOARD_PASSWORD_LEN {
            return Err(MaestroError::Config(format!(
                "[web] dashboard_password exceeds {MAX_DASHBOARD_PASSWORD_LEN} bytes"
            )));
        }

        // Validate CORS origins (normalization is done by `normalize_cors_origins` before validate).
        for (i, origin) in self.web.cors_origins.iter().enumerate() {
            if let Err(msg) = validate_cors_origin(origin) {
                return Err(MaestroError::Config(format!("{msg} (entry index {i})")));
            }
        }

        if self.jira.done_status.trim().is_empty() {
            return Err(MaestroError::Config(
                "[jira] done_status must be non-empty (Jira transition target for Mark as Done)"
                    .to_string(),
            ));
        }

        if self.agent.step_timeout_secs == 0 {
            return Err(MaestroError::Config(
                "step_timeout_secs must be at least 1".to_string(),
            ));
        }

        if self.agent.provider == AiAgentProvider::Cursor
            && self.agent.effective_cursor_cli().trim().is_empty()
        {
            return Err(MaestroError::Config(
                "agent.providers.cursor.cli (or legacy agent.cursor_cli) must be set when agent.provider is \"cursor\""
                    .to_string(),
            ));
        }

        // T-CFG-002 (Phase 1, amendment A1): the Cursor CLI does not honour
        // custom endpoints, so a non-empty `[agent.providers.cursor].base_url`
        // is silently ignored at runtime and would lull the operator into
        // thinking proxying works. Reject loudly at validate time.
        if !self.agent.providers.cursor.base_url.trim().is_empty() {
            return Err(MaestroError::Config(
                "[agent.providers.cursor] base_url: Cursor CLI custom endpoints not supported (remove this key)"
                    .to_string(),
            ));
        }

        // Phase 1 (04_architecture.md §0 D10): deny-list every provider's
        // `extra_args` against Maestro-owned flags, regardless of which
        // provider is currently active. Operators commonly switch providers
        // without re-reading the deny-list, so we validate the static config.
        validate_extra_args(&self.agent.providers.claude.extra_args)?;
        validate_extra_args(&self.agent.providers.cursor.extra_args)?;
        validate_extra_args(&self.agent.providers.codex.extra_args)?;
        validate_extra_args(&self.agent.providers.opencode.extra_args)?;

        // available_providers entries must be parseable provider identifiers.
        for p in &self.agent.available_providers {
            AiAgentProvider::parse(p).map_err(|e| {
                MaestroError::Config(format!("[agent] available_providers: {e}"))
            })?;
        }

        if self.git.remote.trim().is_empty() {
            return Err(MaestroError::Config(
                "git.remote must be a non-empty remote name (e.g. origin)".to_string(),
            ));
        }

        // GitHub App: if any field is set, validate consistency (all-or-nothing for required fields).
        let gh = &self.github;
        let has_id = gh.app_id != 0;
        let has_inst = gh.app_installation_id != 0;
        let has_key_inline = !gh.app_private_key.trim().is_empty();
        let has_key_path = !gh.app_private_key_path.trim().is_empty();
        let has_any = has_id || has_inst || has_key_inline || has_key_path;
        if has_any {
            if !has_id {
                return Err(MaestroError::Config(
                    "[github] app_id must be set (non-zero) when GitHub App auth is configured"
                        .to_string(),
                ));
            }
            if !has_inst {
                return Err(MaestroError::Config(
                    "[github] app_installation_id must be set (non-zero) when GitHub App auth is configured"
                        .to_string(),
                ));
            }
            if !has_key_inline && !has_key_path {
                return Err(MaestroError::Config(
                    "[github] set app_private_key (PEM content) or app_private_key_path (path to PEM file)"
                        .to_string(),
                ));
            }
            if has_key_inline && has_key_path {
                return Err(MaestroError::Config(
                    "[github] set either app_private_key or app_private_key_path, not both"
                        .to_string(),
                ));
            }
        }

        Ok(())
    }

    pub fn to_toml_string(&self) -> Result<String> {
        toml::to_string_pretty(self)
            .map_err(|e| MaestroError::Config(format!("Failed to serialize config: {e}")))
    }

    /// Copy for JSON API responses: strips secrets (never expose via `GET /api/config`).
    pub fn redacted_for_api_clone(&self) -> Self {
        let mut c = self.clone();
        c.web.dashboard_password.clear();
        c.github.app_private_key.clear();
        c.github.app_private_key_path.clear();
        c
    }
}

/// Dashboard `PUT /api/config` body: only these top-level keys are accepted (`deny_unknown_fields`).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeDashboardConfigPatch {
    #[serde(default)]
    pub web: Option<WebLoginPatch>,
    #[serde(default)]
    pub general: Option<GeneralConcurrencyPatch>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebLoginPatch {
    #[serde(default)]
    pub dashboard_username: Option<String>,
    #[serde(default)]
    pub dashboard_password: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneralConcurrencyPatch {
    #[serde(default)]
    pub max_concurrent_workflows: Option<u32>,
    #[serde(default)]
    pub max_active_workflows: Option<u32>,
}

impl Config {
    /// Merge runtime-editable fields from the dashboard. Returns an error if the patch is empty
    /// or leaves the config invalid.
    pub fn apply_runtime_dashboard_patch(
        &mut self,
        patch: RuntimeDashboardConfigPatch,
    ) -> Result<()> {
        let mut applied = false;

        if let Some(ref w) = patch.web {
            let touched = w.dashboard_username.is_some() || w.dashboard_password.is_some();
            if !touched {
                return Err(MaestroError::Config(
                    "\"web\" patch must include dashboard_username and/or dashboard_password"
                        .into(),
                ));
            }
            applied = true;
            if let Some(ref u) = w.dashboard_username {
                self.web.dashboard_username = u.clone();
            }
            if let Some(ref p) = w.dashboard_password {
                if p.is_empty()
                    && !self.web.dashboard_username.trim().is_empty()
                    && !self.web.dashboard_password.is_empty()
                {
                    // preserve existing secret when UI omits password
                } else {
                    self.web.dashboard_password = p.clone();
                }
            }
        }

        if let Some(ref g) = patch.general {
            let touched = g.max_concurrent_workflows.is_some() || g.max_active_workflows.is_some();
            if !touched {
                return Err(MaestroError::Config(
                    "\"general\" patch must include max_concurrent_workflows and/or max_active_workflows"
                        .into(),
                ));
            }
            applied = true;
            if let Some(mc) = g.max_concurrent_workflows {
                self.general.max_concurrent_workflows = mc;
            }
            if let Some(ma) = g.max_active_workflows {
                self.general.max_active_workflows = ma;
            }
        }

        if !applied {
            return Err(MaestroError::Config(
                "empty runtime patch: include \"web\" and/or \"general\" with at least one field"
                    .into(),
            ));
        }

        self.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn valid_config_toml() -> &'static str {
        r#"
[general]
dry_mode = true
auto_polling = false
poll_interval_secs = 30
max_concurrent_workflows = 2

[jira]
project_keys = ["PROJ", "CORE"]
item_types = ["Task", "Bug"]

[git]
base_branch = "main"
repo_path = "/workspace"

[web]
port = 8080

[agent]
step_timeout_secs = 600
"#
    }

    #[test]
    fn test_load_valid_config() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(valid_config_toml().as_bytes()).unwrap();
        let config = Config::load(f.path()).unwrap();
        assert!(config.general.dry_mode);
        assert!(!config.general.auto_polling);
        assert_eq!(config.general.poll_interval_secs, 30);
        assert_eq!(config.jira.project_keys, vec!["PROJ", "CORE"]);
    }

    #[test]
    fn test_load_missing_file() {
        let result = Config::load(Path::new("/nonexistent/config.toml"));
        assert!(result.is_err());
    }

    #[test]
    fn test_defaults() {
        let config = Config::default();
        assert!(!config.general.dry_mode);
        assert!(config.general.auto_polling);
        assert_eq!(config.general.poll_interval_secs, 60);
        assert_eq!(config.web.port, 8080);
        assert!(!config.web.dashboard_auth_enabled());
        assert_eq!(config.agent.cursor_model, "Auto");
        assert_eq!(config.git.remote, "origin");
    }

    #[test]
    fn test_validate_poll_interval_too_low() {
        let mut config = Config::default();
        config.general.poll_interval_secs = 5;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_empty_item_types() {
        let mut config = Config::default();
        config.jira.item_types.clear();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_empty_git_remote() {
        let mut config = Config::default();
        config.git.remote = "   ".to_string();
        assert!(config.validate().is_err());
    }

    #[test]
    fn legacy_commands_table_is_silently_ignored() {
        // Plan-09: stale `[commands]` in a user's config.toml is ignored at
        // load time. The startup warning is logged but the config still
        // parses cleanly (no panic, no error).
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(
            br#"
[general]
poll_interval_secs = 30

[jira]
project_keys = ["X"]
item_types = ["Task"]

[git]
base_branch = "main"

[commands]
worktree_init_commands = ["echo legacy"]
pre_install = ["should be ignored"]

[web]
port = 8080

[agent]
step_timeout_secs = 600
"#,
        )
        .unwrap();
        // Must load without error — the legacy [commands] table is dropped.
        Config::load(f.path()).expect("load must succeed with stale [commands]");
    }

    #[test]
    fn legacy_run_commands_array_is_silently_ignored() {
        // Plan-09: stale `[[run_commands]]` entries are ignored at load time.
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(
            br#"
[general]
poll_interval_secs = 30

[jira]
project_keys = ["X"]
item_types = ["Task"]

[git]
base_branch = "main"

[web]
port = 8080

[agent]
step_timeout_secs = 600

[[run_commands]]
name = "Dev Server"
command = "npm run dev"
"#,
        )
        .unwrap();
        Config::load(f.path()).expect("load must succeed with stale [[run_commands]]");
    }

    #[test]
    fn runtime_patch_json_unknown_top_level_field_fails() {
        let err =
            serde_json::from_str::<RuntimeDashboardConfigPatch>(r#"{"jira":{}}"#).unwrap_err();
        let s = err.to_string();
        assert!(
            s.contains("unknown field") || s.contains("Unknown field"),
            "unexpected serde error: {s}"
        );
    }

    #[test]
    fn runtime_patch_merge_general_only() {
        let mut c = Config::default();
        let patch: RuntimeDashboardConfigPatch =
            serde_json::from_str(r#"{"general":{"max_concurrent_workflows":7}}"#).unwrap();
        c.apply_runtime_dashboard_patch(patch).unwrap();
        assert_eq!(c.general.max_concurrent_workflows, 7);
    }

    #[test]
    fn runtime_patch_empty_top_level_errors() {
        let mut c = Config::default();
        let patch: RuntimeDashboardConfigPatch = serde_json::from_str("{}").unwrap();
        assert!(c.apply_runtime_dashboard_patch(patch).is_err());
    }

    #[test]
    fn runtime_patch_web_empty_subobject_errors() {
        let mut c = Config::default();
        let patch: RuntimeDashboardConfigPatch = serde_json::from_str(r#"{"web":{}}"#).unwrap();
        assert!(c.apply_runtime_dashboard_patch(patch).is_err());
    }

    // -- CORS origin tests --

    #[test]
    fn cors_origins_defaults_to_empty_vec() {
        let config = Config::default();
        assert!(config.web.cors_origins.is_empty());
    }

    #[test]
    fn cors_origins_deserialized_from_toml() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(
            br#"
[general]
poll_interval_secs = 30
[jira]
project_keys = ["X"]
item_types = ["Task"]
[git]
base_branch = "main"
repo_path = "/workspace"
[web]
port = 8080
cors_origins = ["http://example.com:3000"]
[agent]
step_timeout_secs = 600
"#,
        )
        .unwrap();
        let config = Config::load(f.path()).unwrap();
        assert_eq!(config.web.cors_origins, vec!["http://example.com:3000"]);
    }

    #[test]
    fn cors_origins_invalid_in_toml_rejected_by_load() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(
            br#"
[general]
poll_interval_secs = 30
[jira]
project_keys = ["X"]
item_types = ["Task"]
[git]
base_branch = "main"
repo_path = "/workspace"
[web]
port = 8080
cors_origins = ["localhost:3000"]
[agent]
step_timeout_secs = 600
"#,
        )
        .unwrap();
        let err = Config::load(f.path()).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("http://") || msg.contains("https://"),
            "expected scheme error from Config::load, got: {msg}"
        );
    }

    #[test]
    fn cors_origins_omitted_in_toml_defaults_to_empty() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(valid_config_toml().as_bytes()).unwrap();
        let config = Config::load(f.path()).unwrap();
        assert!(config.web.cors_origins.is_empty());
    }

    // -- resolved_cors_origins auto-computation --

    #[test]
    fn resolved_cors_origins_wildcard_includes_all_variants() {
        let web = WebConfig {
            host: "0.0.0.0".into(),
            port: 3000,
            cors_origins: Vec::new(),
            ..Default::default()
        };
        assert_eq!(
            web.resolved_cors_origins(),
            vec![
                "http://localhost:3000",
                "http://127.0.0.1:3000",
                "http://0.0.0.0:3000",
            ]
        );
    }

    #[test]
    fn resolved_cors_origins_ipv6_any_includes_all_variants() {
        let web = WebConfig {
            host: "[::]".into(),
            port: 8080,
            cors_origins: Vec::new(),
            ..Default::default()
        };
        assert_eq!(
            web.resolved_cors_origins(),
            vec![
                "http://localhost:8080",
                "http://127.0.0.1:8080",
                "http://0.0.0.0:8080",
            ]
        );
    }

    #[test]
    fn resolved_cors_origins_127001_includes_localhost() {
        let web = WebConfig {
            host: "127.0.0.1".into(),
            port: 9090,
            cors_origins: Vec::new(),
            ..Default::default()
        };
        assert_eq!(
            web.resolved_cors_origins(),
            vec!["http://localhost:9090", "http://127.0.0.1:9090"]
        );
    }

    #[test]
    fn resolved_cors_origins_ipv6_loopback_includes_localhost() {
        let web = WebConfig {
            host: "::1".into(),
            port: 4000,
            cors_origins: Vec::new(),
            ..Default::default()
        };
        assert_eq!(
            web.resolved_cors_origins(),
            vec!["http://localhost:4000", "http://[::1]:4000"]
        );
    }

    #[test]
    fn resolved_cors_origins_specific_host() {
        let web = WebConfig {
            host: "10.0.0.5".into(),
            port: 8080,
            cors_origins: Vec::new(),
            ..Default::default()
        };
        assert_eq!(web.resolved_cors_origins(), vec!["http://10.0.0.5:8080"]);
    }

    #[test]
    fn resolved_cors_origins_returns_explicit_list() {
        let web = WebConfig {
            host: "0.0.0.0".into(),
            port: 8080,
            cors_origins: vec![
                "https://app.example.com".into(),
                "http://localhost:3000".into(),
            ],
            ..Default::default()
        };
        let resolved = web.resolved_cors_origins();
        assert_eq!(
            resolved,
            vec!["https://app.example.com", "http://localhost:3000"]
        );
    }

    // -- Validation: valid origins --

    #[test]
    fn validate_accepts_http_origin() {
        let mut config = Config::default();
        config.web.cors_origins = vec!["http://localhost:3000".into()];
        assert!(config.validate().is_ok());
    }

    #[test]
    fn validate_accepts_https_origin() {
        let mut config = Config::default();
        config.web.cors_origins = vec!["https://app.example.com".into()];
        assert!(config.validate().is_ok());
    }

    #[test]
    fn validate_accepts_multiple_origins() {
        let mut config = Config::default();
        config.web.cors_origins = vec![
            "http://localhost:3000".into(),
            "https://prod.example.com".into(),
        ];
        assert!(config.validate().is_ok());
    }

    #[test]
    fn validate_accepts_empty_cors_origins() {
        let config = Config::default();
        assert!(config.validate().is_ok());
    }

    // -- Validation: invalid origins --

    #[test]
    fn validate_rejects_origin_without_scheme() {
        let mut config = Config::default();
        config.web.cors_origins = vec!["localhost:3000".into()];
        let err = config.validate().unwrap_err();
        assert!(
            err.to_string().contains("http://") || err.to_string().contains("https://"),
            "expected scheme error: {}",
            err
        );
    }

    #[test]
    fn validate_rejects_ftp_scheme() {
        let mut config = Config::default();
        config.web.cors_origins = vec!["ftp://files.example.com".into()];
        let err = config.validate().unwrap_err();
        assert!(
            err.to_string().contains("http://") || err.to_string().contains("https://"),
            "expected scheme error: {}",
            err
        );
    }

    #[test]
    fn validate_rejects_origin_with_path() {
        let mut config = Config::default();
        config.web.cors_origins = vec!["http://localhost:3000/api".into()];
        let err = config.validate().unwrap_err();
        assert!(
            err.to_string().contains("path"),
            "expected path error: {}",
            err
        );
    }

    #[test]
    fn validate_rejects_origin_with_trailing_slash() {
        let mut config = Config::default();
        config.web.cors_origins = vec!["http://localhost:3000/".into()];
        let err = config.validate().unwrap_err();
        assert!(
            err.to_string().contains("path"),
            "expected path error: {}",
            err
        );
    }

    #[test]
    fn validate_rejects_empty_string_origin() {
        let mut config = Config::default();
        config.web.cors_origins = vec!["".into()];
        let err = config.validate().unwrap_err();
        assert!(
            err.to_string().contains("empty"),
            "expected empty error: {}",
            err
        );
    }

    #[test]
    fn validate_rejects_whitespace_only_origin() {
        let mut config = Config::default();
        config.web.cors_origins = vec!["   ".into()];
        let err = config.validate().unwrap_err();
        assert!(
            err.to_string().contains("empty"),
            "expected empty error: {}",
            err
        );
    }

    #[test]
    fn validate_rejects_if_any_origin_invalid() {
        let mut config = Config::default();
        config.web.cors_origins = vec!["http://localhost:3000".into(), "bad".into()];
        assert!(config.validate().is_err());
    }

    // -- Normalization --

    #[test]
    fn normalize_cors_origins_strips_http_port_80() {
        let mut web = WebConfig {
            cors_origins: vec!["http://example.com:80".into()],
            ..Default::default()
        };
        web.normalize_cors_origins();
        assert_eq!(web.cors_origins, vec!["http://example.com"]);
    }

    #[test]
    fn normalize_cors_origins_strips_https_port_443() {
        let mut web = WebConfig {
            cors_origins: vec!["https://example.com:443".into()],
            ..Default::default()
        };
        web.normalize_cors_origins();
        assert_eq!(web.cors_origins, vec!["https://example.com"]);
    }

    #[test]
    fn normalize_cors_origins_preserves_non_default_port() {
        let mut web = WebConfig {
            cors_origins: vec!["http://example.com:8080".into()],
            ..Default::default()
        };
        web.normalize_cors_origins();
        assert_eq!(web.cors_origins, vec!["http://example.com:8080"]);
    }

    // -- Redaction --

    #[test]
    fn redacted_clone_preserves_cors_origins() {
        let mut config = Config::default();
        config.web.cors_origins = vec!["http://localhost:3000".into()];
        let redacted = config.redacted_for_api_clone();
        assert_eq!(redacted.web.cors_origins, vec!["http://localhost:3000"]);
    }

    // -- Runtime patch rejection --

    #[test]
    fn runtime_patch_rejects_cors_origins_field() {
        let err = serde_json::from_str::<RuntimeDashboardConfigPatch>(
            r#"{"web":{"cors_origins":["http://x"]}}"#,
        )
        .unwrap_err();
        let s = err.to_string();
        assert!(
            s.contains("unknown field") || s.contains("Unknown field"),
            "expected unknown field error: {s}"
        );
    }

    // -- generate_report --

    #[test]
    fn generate_report_defaults_to_false() {
        let config = Config::default();
        assert!(!config.general.generate_report);
    }

    #[test]
    fn generate_report_true_from_toml() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(
            br#"
[general]
generate_report = true
poll_interval_secs = 30

[jira]
project_keys = ["X"]
item_types = ["Task"]

[git]
base_branch = "main"
repo_path = "/workspace"

[web]
port = 8080

[agent]
step_timeout_secs = 600
"#,
        )
        .unwrap();
        let config = Config::load(f.path()).unwrap();
        assert!(config.general.generate_report);
    }

    #[test]
    fn generate_report_false_when_omitted() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(valid_config_toml().as_bytes()).unwrap();
        let config = Config::load(f.path()).unwrap();
        assert!(!config.general.generate_report);
    }

    // ─── Phase 1: provider sub-tables, migration, validation ─────────────

    #[test]
    fn load_migrates_legacy_cursor_cli_to_subtable() {
        let toml = r#"
[general]
poll_interval_secs = 30
max_concurrent_workflows = 2

[jira]
project_keys = ["X"]
item_types = ["Task"]

[git]
base_branch = "main"

[web]
port = 8080

[agent]
provider = "cursor"
cursor_cli = "agent-custom"
cursor_model = "gpt-4.1"
model = "claude-3-5"
"#;
        let cfg = Config::load_from_str(toml).expect("load");
        assert_eq!(cfg.agent.providers.cursor.cli, "agent-custom");
        assert_eq!(cfg.agent.providers.cursor.model, "gpt-4.1");
        // `agent.model` migrates into the **active** provider's sub-table.
        assert_eq!(cfg.agent.providers.cursor.model, "gpt-4.1");
    }

    #[test]
    fn load_with_subtable_does_not_overwrite_explicit_sub_value() {
        // When both the legacy field and the sub-table value are set, the
        // sub-table wins (migration is "fill if empty").
        let toml = r#"
[general]
poll_interval_secs = 30
max_concurrent_workflows = 2

[jira]
project_keys = ["X"]
item_types = ["Task"]

[git]
base_branch = "main"

[web]
port = 8080

[agent]
provider = "cursor"
cursor_cli = "legacy-agent"

[agent.providers.cursor]
cli = "sub-table-agent"
"#;
        let cfg = Config::load_from_str(toml).expect("load");
        assert_eq!(cfg.agent.providers.cursor.cli, "sub-table-agent");
    }

    /// T-CFG-002 (Phase 1, P1): a non-empty `[agent.providers.cursor].base_url`
    /// is rejected with a stable, user-visible message — Cursor's CLI does not
    /// honour custom endpoints (amendment A1).
    #[test]
    fn load_rejects_cursor_base_url_with_friendly_message() {
        let toml = r#"
[general]
poll_interval_secs = 30
max_concurrent_workflows = 2

[jira]
project_keys = ["X"]
item_types = ["Task"]

[git]
base_branch = "main"

[web]
port = 8080

[agent]
provider = "claude"

[agent.providers.cursor]
base_url = "https://proxy.example.com"
"#;
        let err = Config::load_from_str(toml).expect_err("cursor base_url must reject");
        let msg = err.to_string();
        assert!(
            msg.contains("Cursor CLI custom endpoints not supported"),
            "expected friendly message, got: {msg}"
        );
    }

    /// Empty / default `[agent.providers.cursor].base_url` continues to load
    /// (the validator only fires on non-empty values) — guarantees the new
    /// check doesn't break clean configs.
    #[test]
    fn load_accepts_empty_cursor_base_url() {
        let toml = r#"
[general]
poll_interval_secs = 30
max_concurrent_workflows = 2

[jira]
project_keys = ["X"]
item_types = ["Task"]

[git]
base_branch = "main"

[web]
port = 8080

[agent]
provider = "claude"

[agent.providers.cursor]
base_url = ""
"#;
        Config::load_from_str(toml).expect("empty cursor.base_url must load");
    }

    #[test]
    fn load_rejects_denied_extra_arg_in_subtable() {
        let toml = r#"
[general]
poll_interval_secs = 30
max_concurrent_workflows = 2

[jira]
project_keys = ["X"]
item_types = ["Task"]

[git]
base_branch = "main"

[web]
port = 8080

[agent]
provider = "claude"

[agent.providers.claude]
extra_args = ["--dangerously-skip-permissions"]
"#;
        let err = Config::load_from_str(toml).expect_err("denied flag must reject");
        let msg = err.to_string();
        assert!(
            msg.contains("extra_args_denied"),
            "expected extra_args_denied in error, got: {msg}"
        );
    }

    #[test]
    fn load_rejects_unknown_available_provider() {
        let toml = r#"
[general]
poll_interval_secs = 30
max_concurrent_workflows = 2

[jira]
project_keys = ["X"]
item_types = ["Task"]

[git]
base_branch = "main"

[web]
port = 8080

[agent]
provider = "claude"
available_providers = ["claude", "bogus"]
"#;
        let err = Config::load_from_str(toml).expect_err("unknown provider must reject");
        assert!(err.to_string().contains("bogus"));
    }

    #[test]
    fn default_available_providers_lists_all_v1() {
        let cfg = Config::default();
        assert_eq!(
            cfg.agent.available_providers,
            vec!["claude", "cursor", "codex", "opencode"]
        );
    }

    #[test]
    fn to_toml_round_trip_preserves_provider_sub_tables() {
        let mut cfg = Config::default();
        cfg.agent.providers.claude.model = "claude-3-5".into();
        cfg.agent.providers.claude.base_url = "https://proxy.example.com".into();
        cfg.agent.providers.cursor.cli = "agent-custom".into();
        cfg.agent.providers.cursor.model = "gpt-4.1".into();
        cfg.agent.providers.codex.provider_name = "lmstudio".into();
        cfg.agent.providers.codex.base_url = "http://lm-studio:1234/v1".into();
        cfg.agent.providers.opencode.model = "anthropic/claude-3-5-sonnet".into();

        let serialized = cfg.to_toml_string().expect("serialize");
        let parsed: Config = toml::from_str(&serialized).expect("re-parse");

        assert_eq!(parsed.agent.providers.claude.model, "claude-3-5");
        assert_eq!(
            parsed.agent.providers.claude.base_url,
            "https://proxy.example.com"
        );
        assert_eq!(parsed.agent.providers.cursor.cli, "agent-custom");
        assert_eq!(parsed.agent.providers.cursor.model, "gpt-4.1");
        assert_eq!(parsed.agent.providers.codex.provider_name, "lmstudio");
        assert_eq!(
            parsed.agent.providers.codex.base_url,
            "http://lm-studio:1234/v1"
        );
        assert_eq!(
            parsed.agent.providers.opencode.model,
            "anthropic/claude-3-5-sonnet"
        );
    }

    #[test]
    fn codex_provider_serde_round_trips_lowercase() {
        let cfg: Config = toml::from_str(
            r#"
[general]
poll_interval_secs = 30
max_concurrent_workflows = 2
[jira]
project_keys = ["X"]
item_types = ["Task"]
[git]
base_branch = "main"
[web]
port = 8080
[agent]
provider = "codex"
"#,
        )
        .expect("parse");
        assert_eq!(cfg.agent.provider, AiAgentProvider::Codex);
        assert_eq!(cfg.agent.provider.as_str(), "codex");
    }

    #[test]
    fn opencode_provider_serde_round_trips_lowercase() {
        let cfg: Config = toml::from_str(
            r#"
[general]
poll_interval_secs = 30
max_concurrent_workflows = 2
[jira]
project_keys = ["X"]
item_types = ["Task"]
[git]
base_branch = "main"
[web]
port = 8080
[agent]
provider = "opencode"
"#,
        )
        .expect("parse");
        assert_eq!(cfg.agent.provider, AiAgentProvider::OpenCode);
        assert_eq!(cfg.agent.provider.as_str(), "opencode");
    }

    // ─── Task #48: provisioning_sha ─────────────────────────────────────

    fn config_with_provisioning(cmds: &[&str]) -> Config {
        let mut cfg = Config::default();
        cfg.provisioning.install_commands =
            cmds.iter().map(|s| s.to_string()).collect();
        cfg
    }

    /// T-PROV-SHA-001: same list → same SHA (the boot-side fast-path
    /// gate works deterministically across restarts and machines).
    #[test]
    fn provisioning_sha_is_stable_for_same_content() {
        let a = config_with_provisioning(&["cmd-1", "cmd-2"]);
        let b = config_with_provisioning(&["cmd-1", "cmd-2"]);
        assert_eq!(a.provisioning_sha(), b.provisioning_sha());
        // And it's not a random uuid pretending to be a SHA — must be
        // 64 lowercase hex chars (sha256 hex digest).
        let sha = a.provisioning_sha();
        assert_eq!(sha.len(), 64, "sha must be 64 hex chars; got {sha}");
        assert!(sha.chars().all(|c| c.is_ascii_hexdigit()
            && (!c.is_ascii_alphabetic() || c.is_ascii_lowercase())));
    }

    /// T-PROV-SHA-002: edit a command → SHA changes (cache invalidation).
    #[test]
    fn provisioning_sha_changes_when_command_text_changes() {
        let a = config_with_provisioning(&["install foo"]);
        let b = config_with_provisioning(&["install bar"]);
        assert_ne!(a.provisioning_sha(), b.provisioning_sha());
    }

    /// T-PROV-SHA-003: order matters (later commands can depend on
    /// artifacts from earlier ones — `[a, b]` is NOT the same install
    /// as `[b, a]`). The SHA must reflect that.
    #[test]
    fn provisioning_sha_order_sensitive() {
        let a = config_with_provisioning(&["cmd-1", "cmd-2"]);
        let b = config_with_provisioning(&["cmd-2", "cmd-1"]);
        assert_ne!(a.provisioning_sha(), b.provisioning_sha());
    }

    /// T-PROV-SHA-004: empty list yields a known stable SHA so the
    /// entrypoint can fast-path-skip even on the empty-config case
    /// without re-running every boot.
    #[test]
    fn provisioning_sha_empty_list_is_stable_known_value() {
        let cfg = Config::default();
        assert!(cfg.provisioning.install_commands.is_empty());
        // sha256 of `[]` (the JSON-encoded empty array) is well-known.
        // Recompute on the fly so a change to the canonicalization
        // scheme fails this test loudly rather than silently shifting
        // the gate value.
        use sha2::{Digest, Sha256};
        let expected = format!("{:x}", Sha256::digest(b"[]"));
        assert_eq!(cfg.provisioning_sha(), expected);
    }

    /// T-PROV-SHA-005: whitespace inside a command is part of the
    /// command's identity — the canonicalizer must NOT collapse spaces
    /// (admins may rely on multi-space formatting inside a heredoc /
    /// args list).
    #[test]
    fn provisioning_sha_preserves_inner_whitespace() {
        let a = config_with_provisioning(&["cmd  --flag  value"]);
        let b = config_with_provisioning(&["cmd --flag value"]);
        assert_ne!(a.provisioning_sha(), b.provisioning_sha());
    }
}
