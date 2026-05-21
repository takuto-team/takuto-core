// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Legacy `[agent]` flat-field migration + `effective_cursor_*` accessors.
//!
//! Split out of `agent.rs` to keep that file ≤600 LOC per the PO plan. These
//! helpers all exist for one release of back-compat with pre-Phase-1
//! `cursor_cli` / `cursor_model` / `model` keys at the top of `[agent]`.

use super::{AgentConfig, AiAgentProvider};

pub(super) fn default_cursor_cli() -> String {
    "agent".to_string()
}

pub(super) fn default_cursor_model() -> String {
    "Auto".to_string()
}

impl AgentConfig {
    /// Phase 1 migration (04_architecture.md §8): copy values from the legacy
    /// flat `[agent].cursor_cli` / `cursor_model` / `model` fields into the
    /// new `[agent.providers.<name>]` sub-tables when the sub-table key is
    /// empty. Idempotent — running it twice produces the same result.
    ///
    /// Emits a `tracing::warn!` per migrated key. The file is **not** rewritten
    /// at load time; the next save via `ConfigWriter` writes the new shape.
    pub fn migrate_legacy_flat_fields(&mut self) {
        // Cursor: flat cursor_cli → providers.cursor.cli. Migrate when the
        // sub-table is empty AND the legacy field carries a non-default
        // value. The legacy default ("agent") is also `effective_cursor_cli`'s
        // fallback when the sub-table is empty, so skipping migration in
        // that case keeps the on-disk shape minimal.
        if self.providers.cursor.cli.trim().is_empty()
            && !self.cursor_cli.trim().is_empty()
            && self.cursor_cli != default_cursor_cli()
        {
            tracing::warn!(
                from = "agent.cursor_cli",
                to = "agent.providers.cursor.cli",
                "config: legacy field migrated to [agent.providers.cursor]"
            );
            self.providers.cursor.cli = self.cursor_cli.clone();
        }
        // Cursor: flat cursor_model → providers.cursor.model.
        if self.providers.cursor.model.trim().is_empty()
            && !self.cursor_model.trim().is_empty()
            && self.cursor_model != default_cursor_model()
        {
            tracing::warn!(
                from = "agent.cursor_model",
                to = "agent.providers.cursor.model",
                "config: legacy field migrated to [agent.providers.cursor]"
            );
            self.providers.cursor.model = self.cursor_model.clone();
        }
        // Generic model: flat agent.model → providers.<active>.model.
        if !self.model.trim().is_empty() {
            let dest = match self.provider {
                AiAgentProvider::Claude => &mut self.providers.claude.model,
                AiAgentProvider::Cursor => &mut self.providers.cursor.model,
                AiAgentProvider::Codex => &mut self.providers.codex.model,
                AiAgentProvider::OpenCode => &mut self.providers.opencode.model,
            };
            if dest.trim().is_empty() {
                tracing::warn!(
                    from = "agent.model",
                    to = "agent.providers.<active>.model",
                    provider = %self.provider.as_str(),
                    "config: legacy field migrated to active provider sub-table"
                );
                *dest = self.model.clone();
            }
        }
    }

    /// Return the effective Cursor CLI binary, preferring the sub-table value
    /// when non-empty, then the legacy flat field, then the hard-coded default.
    pub fn effective_cursor_cli(&self) -> &str {
        let sub = self.providers.cursor.cli.trim();
        if !sub.is_empty() {
            return &self.providers.cursor.cli;
        }
        let legacy = self.cursor_cli.trim();
        if !legacy.is_empty() {
            return &self.cursor_cli;
        }
        "agent"
    }

    /// Return the effective Cursor model, preferring the sub-table value
    /// when non-empty, then the legacy flat field, then the hard-coded default.
    pub fn effective_cursor_model(&self) -> &str {
        let sub = self.providers.cursor.model.trim();
        if !sub.is_empty() {
            return &self.providers.cursor.model;
        }
        let legacy = self.cursor_model.trim();
        if !legacy.is_empty() {
            return &self.cursor_model;
        }
        "Auto"
    }
}
