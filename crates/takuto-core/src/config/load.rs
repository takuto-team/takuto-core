// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.
//! `Config::load` / `Config::load_from_str` / `Config::validate` and the
//! adjacent path-resolution + legacy-key-detection helpers. Split out of
//! `mod.rs` to keep the facade ≤ 200 LOC per the PO plan.

use std::path::{Path, PathBuf};

use crate::config::ConfigError;
use crate::error::{Result, TakutoError};

use super::{AiAgentProvider, Config, validate_cors_origin, validate_extra_args};

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
/// any time tracing is initialised; on first load the caller in `takuto-cli`
/// uses [`detect_legacy_command_keys`] directly and replays the warnings
/// after tracing setup.
pub(super) fn warn_legacy_command_keys(toml_content: &str) {
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
        config.validate()?;
        Ok(config)
    }

    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Err(TakutoError::ConfigNotFound(path.to_path_buf()));
        }

        let content = std::fs::read_to_string(path)?;
        warn_legacy_command_keys(&content);
        let mut config: Config = toml::from_str(&content)?;
        config.web.normalize_cors_origins();
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        if self.general.poll_interval_secs < 10 {
            return Err(ConfigError::Validation {
                section: "general",
                field: "poll_interval_secs",
                detail: "must be at least 10".to_string(),
            }
            .into());
        }

        if self.general.max_concurrent_workflows == 0 {
            return Err(ConfigError::Validation {
                section: "general",
                field: "max_concurrent_workflows",
                detail: "must be at least 1".to_string(),
            }
            .into());
        }

        if self.general.effective_max_active_workflows() < 1 {
            return Err(ConfigError::Validation {
                section: "general",
                field: "max_active_workflows",
                detail: "must be at least 1 when set, or leave 0 to use max_concurrent_workflows"
                    .to_string(),
            }
            .into());
        }

        for key in &self.jira.project_keys {
            if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric()) {
                return Err(ConfigError::Validation {
                    section: "jira",
                    field: "project_keys",
                    detail: format!(
                        "invalid key '{key}': must be non-empty uppercase alphanumeric"
                    ),
                }
                .into());
            }
        }

        if self.jira.item_types.is_empty() {
            return Err(ConfigError::Validation {
                section: "jira",
                field: "item_types",
                detail: "at least one Jira item type must be configured".to_string(),
            }
            .into());
        }

        if self.web.port == 0 {
            return Err(ConfigError::Validation {
                section: "web",
                field: "port",
                detail: "must be a non-zero value".to_string(),
            }
            .into());
        }

        // Validate CORS origins (normalization is done by `normalize_cors_origins` before validate).
        for (i, origin) in self.web.cors_origins.iter().enumerate() {
            if let Err(msg) = validate_cors_origin(origin) {
                return Err(ConfigError::Validation {
                    section: "web",
                    field: "cors_origins",
                    detail: format!("{msg} (entry index {i})"),
                }
                .into());
            }
        }

        if self.jira.done_status.trim().is_empty() {
            return Err(ConfigError::Validation {
                section: "jira",
                field: "done_status",
                detail: "must be non-empty (Jira transition target for Mark as Done)".to_string(),
            }
            .into());
        }

        if self.agent.step_timeout_secs == 0 {
            return Err(ConfigError::Validation {
                section: "agent",
                field: "step_timeout_secs",
                detail: "must be at least 1".to_string(),
            }
            .into());
        }

        if self.agent.provider == AiAgentProvider::Cursor
            && self.agent.effective_cursor_cli().trim().is_empty()
        {
            return Err(ConfigError::Validation {
                section: "agent",
                field: "providers.cursor.cli",
                detail: "must be set when agent.provider is \"cursor\"".to_string(),
            }
            .into());
        }

        // T-CFG-002 (Phase 1, amendment A1): the Cursor CLI does not honour
        // custom endpoints, so a non-empty `[agent.providers.cursor].base_url`
        // is silently ignored at runtime and would lull the operator into
        // thinking proxying works. Reject loudly at validate time.
        if !self.agent.providers.cursor.base_url.trim().is_empty() {
            return Err(ConfigError::Validation {
                section: "agent.providers.cursor",
                field: "base_url",
                detail: "Cursor CLI custom endpoints not supported (remove this key)".to_string(),
            }
            .into());
        }

        // OpenCode self-hosted spec (2026-05-27): OpenCode in v1 is the
        // self-hosted-only adapter (LM Studio / Ollama / vLLM / private
        // gateways). There is no sensible default endpoint — the whole
        // point of picking OpenCode is the admin pointing at their own
        // OpenAI-compatible server. An empty `base_url` produces a
        // "no provider configured" error at first workflow step; reject
        // loudly here so the operator sees a typed 400 at config save
        // time and fixes it before the workflow fails.
        //
        // Same reasoning applies to `model`: the OpenCode init-shim writes
        // `models.<id>` into the worker's `opencode.json` and the CLI's
        // `-m <provider>/<model>` argv references that id. Empty → broken.
        if self.agent.provider == AiAgentProvider::OpenCode {
            if self.agent.providers.opencode.base_url.trim().is_empty() {
                return Err(ConfigError::Validation {
                    section: "agent.providers.opencode",
                    field: "base_url",
                    detail: "opencode_base_url_required: set the OpenAI-compatible \
                             endpoint URL for your self-hosted model server \
                             (e.g. http://lm-studio:1234/v1)"
                        .to_string(),
                }
                .into());
            }
            if self.agent.providers.opencode.model.trim().is_empty() {
                return Err(ConfigError::Validation {
                    section: "agent.providers.opencode",
                    field: "model",
                    detail: "opencode_model_required: set the model id served by \
                             your endpoint (e.g. lmstudio/qwen3-coder)"
                        .to_string(),
                }
                .into());
            }
        }

        // OpenCode context/output limits, when present, must be positive — a
        // zero-token window is nonsensical and would emit an invalid `limit`
        // block into the worker's opencode.json. Validated regardless of the
        // active provider so a stored 0 surfaces at save time.
        if self.agent.providers.opencode.context_limit == Some(0) {
            return Err(ConfigError::Validation {
                section: "agent.providers.opencode",
                field: "context_limit",
                detail: "opencode_context_limit_positive: context_limit must be \
                         greater than 0 (leave unset to let OpenCode choose)"
                    .to_string(),
            }
            .into());
        }
        if self.agent.providers.opencode.output_limit == Some(0) {
            return Err(ConfigError::Validation {
                section: "agent.providers.opencode",
                field: "output_limit",
                detail: "opencode_output_limit_positive: output_limit must be \
                         greater than 0 (leave unset to let OpenCode choose)"
                    .to_string(),
            }
            .into());
        }

        // Phase 1 (04_architecture.md §0 D10): deny-list every provider's
        // `extra_args` against Takuto-owned flags, regardless of which
        // provider is currently active. Operators commonly switch providers
        // without re-reading the deny-list, so we validate the static config.
        validate_extra_args(&self.agent.providers.claude.extra_args)?;
        validate_extra_args(&self.agent.providers.cursor.extra_args)?;
        validate_extra_args(&self.agent.providers.codex.extra_args)?;
        validate_extra_args(&self.agent.providers.opencode.extra_args)?;

        // available_providers entries must be parseable provider identifiers.
        for p in &self.agent.available_providers {
            AiAgentProvider::parse(p).map_err(|e| ConfigError::Validation {
                section: "agent",
                field: "available_providers",
                detail: e.to_string(),
            })?;
        }

        if self.git.remote.trim().is_empty() {
            return Err(ConfigError::Validation {
                section: "git",
                field: "remote",
                detail: "must be a non-empty remote name (e.g. origin)".to_string(),
            }
            .into());
        }

        // Polling filters: reject empty / whitespace-only entries so a stray
        // `""` cannot silently degrade a filter into match-everything. No
        // validation on `auto_start_flow` (slug resolved per-workspace/user at
        // runtime) or `max_parallel_items` (`0` = unlimited is valid).
        for kw in &self.polling.jira.summary_keywords {
            if kw.trim().is_empty() {
                return Err(ConfigError::Validation {
                    section: "polling.jira",
                    field: "summary_keywords",
                    detail: "entries must be non-empty (remove blank keywords)".to_string(),
                }
                .into());
            }
        }
        for label in &self.polling.github.labels {
            if label.trim().is_empty() {
                return Err(ConfigError::Validation {
                    section: "polling.github",
                    field: "labels",
                    detail: "entries must be non-empty (remove blank labels)".to_string(),
                }
                .into());
            }
        }
        for kw in &self.polling.github.title_keywords {
            if kw.trim().is_empty() {
                return Err(ConfigError::Validation {
                    section: "polling.github",
                    field: "title_keywords",
                    detail: "entries must be non-empty (remove blank keywords)".to_string(),
                }
                .into());
            }
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
                return Err(ConfigError::Validation {
                    section: "github",
                    field: "app_id",
                    detail: "must be set (non-zero) when GitHub App auth is configured".to_string(),
                }
                .into());
            }
            if !has_inst {
                return Err(ConfigError::Validation {
                    section: "github",
                    field: "app_installation_id",
                    detail: "must be set (non-zero) when GitHub App auth is configured".to_string(),
                }
                .into());
            }
            if !has_key_inline && !has_key_path {
                return Err(ConfigError::Validation {
                    section: "github",
                    field: "app_private_key/app_private_key_path",
                    detail: "set app_private_key (PEM content) or app_private_key_path (path to PEM file)".to_string(),
                }
                .into());
            }
            if has_key_inline && has_key_path {
                return Err(ConfigError::Validation {
                    section: "github",
                    field: "app_private_key/app_private_key_path",
                    detail: "set either app_private_key or app_private_key_path, not both"
                        .to_string(),
                }
                .into());
            }
        }

        Ok(())
    }

    pub fn to_toml_string(&self) -> Result<String> {
        toml::to_string_pretty(self).map_err(|source| ConfigError::SerializeToml { source }.into())
    }

    /// Copy for JSON API responses: strips secrets (never expose via `GET /api/config`).
    pub fn redacted_for_api_clone(&self) -> Self {
        let mut c = self.clone();
        c.github.app_private_key.clear();
        c.github.app_private_key_path.clear();
        // `[database].connection` may carry a password
        // (`postgres://user:pw@host/db`). Redact only the password component;
        // operators still need to see the rest of the URL to verify they're
        // pointing at the intended host.
        if !c.database.connection.is_empty() {
            c.database.connection =
                crate::config::redact_connection_password(&c.database.connection);
        }
        c
    }
}
