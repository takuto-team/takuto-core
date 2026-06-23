// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Thin facade for the configuration module. Per-section types live in the
//! sibling sub-modules below; this file owns only the aggregate `Config`
//! struct, the `provisioning_sha` helper that depends on `[provisioning]`,
//! and the `pub use` re-exports that preserve every public path the rest of
//! the workspace expects.

use serde::{Deserialize, Serialize};

mod agent;
pub mod database;
mod egress;
pub mod error;
mod general;
mod git;
mod jira;
mod load;
mod patches;
mod polling;
mod runtime;
mod template;
mod web;

pub use database::{DatabaseConfig, redact_connection_password};
pub use error::ConfigError;

pub use agent::{
    AgentConfig, AgentProviderConfig, AgentProvidersConfig, AgentStepConfig, AiAgentProvider,
    CodexProviderConfig, CursorProviderConfig, DENIED_EXTRA_ARG_FLAGS, OpenCodeProviderConfig,
    SkillRef, StepAvailability, cursor_model_for_cli, validate_extra_args,
};
pub use general::{DevConfig, DockerConfig, GeneralConfig, ProvisioningConfig, TicketingSystem};
pub use git::{GitConfig, GitHubAppConfig};
pub use jira::{JiraConfig, LinkedItemsPromptMode};
pub use load::{detect_legacy_command_keys, resolve_config_relative_path};
pub use polling::matches_any_keyword;
pub use runtime::{EditorConfig, NetworkConfig, TerminalConfig};
pub use template::{interpolate_agent_prompt, interpolate_command_template};
pub use web::{
    GeneralConcurrencyPatch, RuntimeDashboardConfigPatch, WebConfig, validate_cors_origin,
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
    /// Admin-supplied tool installs that run at takuto startup and
    /// populate the shared `takuto-tools` volume. See
    /// [`ProvisioningConfig`].
    #[serde(default)]
    pub provisioning: ProvisioningConfig,
    /// Pluggable database backend. Empty/omitted → default SQLite at
    /// `{data_dir}/takuto.db`; `postgres://…` / `mysql://…` switches the
    /// deployment to that backend.
    #[serde(default)]
    pub database: DatabaseConfig,
}

impl Config {
    /// Canonical sha256 of the `[provisioning].install_commands`
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

#[cfg(test)]
mod tests;
