// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{MaestroError, Result};

mod agent;
mod general;
mod git;
mod jira;
mod runtime;
mod template;
mod web;

pub use agent::{
    AgentConfig, AgentProviderConfig, AgentProvidersConfig, AgentStepConfig, AiAgentProvider,
    CodexProviderConfig, CursorProviderConfig, DENIED_EXTRA_ARG_FLAGS, SkillRef, StepAvailability,
    cursor_model_for_cli, validate_extra_args,
};
pub use general::{
    DevConfig, DockerConfig, GeneralConfig, ProvisioningConfig, TicketingSystem,
};
pub use git::{GitConfig, GitHubAppConfig};
pub use jira::{JiraConfig, LinkedItemsPromptMode};
pub use runtime::{EditorConfig, NetworkConfig, TerminalConfig};
pub use template::{interpolate_agent_prompt, interpolate_command_template};
pub use web::{
    GeneralConcurrencyPatch, RuntimeDashboardConfigPatch, WebConfig, WebLoginPatch,
    validate_cors_origin,
};

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
mod tests;
