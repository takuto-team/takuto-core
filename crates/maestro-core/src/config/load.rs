// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! `Config::load` / `Config::load_from_str` / `Config::validate` and the
//! adjacent path-resolution + legacy-key-detection helpers. Split out of
//! `mod.rs` to keep the facade ≤ 200 LOC per the PO plan.

use std::path::{Path, PathBuf};

use crate::error::{MaestroError, Result};

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
/// any time tracing is initialised; on first load the caller in `maestro-cli`
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
