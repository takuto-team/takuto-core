// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.
//! Agent (AI provider) configuration: provider selection, per-provider sub-tables,
//! per-step config, model resolution, and `extra_args` validation.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::config::ConfigError;
use crate::error::Result;

/// Which CLI implements ticket implementation / review / fix steps.
///
/// Per 04_architecture.md §0 D1 / A1 / A2: four native adapters in v1.
/// All four (Claude, Cursor, Codex, OpenCode) are wired as of Phase 4.
/// The LM Studio recipe is anchored under OpenCode (A2).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum AiAgentProvider {
    #[default]
    Claude,
    Cursor,
    Codex,
    OpenCode,
}

impl AiAgentProvider {
    /// Stable lowercase identifier matching the TOML serde representation.
    pub fn as_str(self) -> &'static str {
        match self {
            AiAgentProvider::Claude => "claude",
            AiAgentProvider::Cursor => "cursor",
            AiAgentProvider::Codex => "codex",
            AiAgentProvider::OpenCode => "opencode",
        }
    }

    /// Parse from the lowercase TOML / API identifier.
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "claude" => Ok(AiAgentProvider::Claude),
            "cursor" => Ok(AiAgentProvider::Cursor),
            "codex" => Ok(AiAgentProvider::Codex),
            "opencode" => Ok(AiAgentProvider::OpenCode),
            other => Err(ConfigError::Validation {
                section: "agent",
                field: "provider",
                detail: format!(
                    "unknown provider \"{other}\" (expected one of: claude, cursor, codex, opencode)"
                ),
            }
            .into()),
        }
    }

    /// `true` for providers whose runtime adapter is wired. All four v1
    /// providers (Claude, Cursor, Codex, OpenCode) are implemented as of
    /// Phase 4.
    pub fn is_runtime_implemented(self) -> bool {
        matches!(
            self,
            AiAgentProvider::Claude
                | AiAgentProvider::Cursor
                | AiAgentProvider::Codex
                | AiAgentProvider::OpenCode
        )
    }

    /// Human-readable label used in error messages and logs.
    /// Matches the legacy `MaestroError::AiAgent(String)` Display prefixes
    /// (e.g. "Cursor Agent exited with code …").
    pub fn display_name(self) -> &'static str {
        match self {
            AiAgentProvider::Claude => "Claude Code",
            AiAgentProvider::Cursor => "Cursor Agent",
            AiAgentProvider::Codex => "Codex CLI",
            AiAgentProvider::OpenCode => "OpenCode",
        }
    }
}

impl fmt::Display for AiAgentProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.display_name())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    #[serde(default)]
    pub provider: AiAgentProvider,
    /// **Deprecated (Phase 1)**: legacy top-level Cursor CLI binary. Moved to
    /// `[agent.providers.cursor].cli`. Still parsed for one release; migrated
    /// at load time and overwritten on next save (see `Config::load`).
    #[serde(default = "super::agent_legacy::default_cursor_cli")]
    pub cursor_cli: String,
    /// **Deprecated (Phase 1)**: legacy top-level Cursor model. Moved to
    /// `[agent.providers.cursor].model`. Migrated at load time.
    #[serde(default = "super::agent_legacy::default_cursor_model")]
    pub cursor_model: String,
    /// Timeout per agent session (applies to all providers).
    #[serde(default = "default_step_timeout")]
    pub step_timeout_secs: u64,
    /// Timeout in seconds for "Improve with AI" / "Prompt ticket" sessions. Default 300.
    #[serde(default = "default_improve_timeout")]
    pub improve_timeout_secs: u64,
    /// **Deprecated (Phase 1)**: legacy top-level model. Moved to
    /// `[agent.providers.<active>].model`. Migrated at load time.
    #[serde(default)]
    pub model: String,
    /// Phase 1: per-provider sub-tables (`[agent.providers.<name>]`). Defaults
    /// are used when the TOML section is missing.
    #[serde(default)]
    pub providers: AgentProvidersConfig,
    /// Phase 1: admin's whitelist of providers users may authenticate against.
    /// Empty = only the active provider is offered. Defaults to all v1
    /// providers (`["claude", "cursor", "codex", "opencode"]`).
    #[serde(default = "default_available_providers")]
    pub available_providers: Vec<String>,
    /// When `true`, every step in a flow shares ONE agent conversation:
    /// each step resumes the prior step's session, so the agent carries
    /// full context forward (it remembers what it implemented when it
    /// reviews, etc.). When `false` (default), each step runs in a fresh
    /// session with no memory of earlier steps — the historical behavior,
    /// and safer for weaker models that get confused by long transcripts.
    /// A per-step `resume_previous = true` still forces resume regardless
    /// of this global setting.
    #[serde(default = "default_share_conversation")]
    pub share_conversation_across_steps: bool,
}

pub(super) fn default_share_conversation() -> bool {
    false
}

pub(super) fn default_available_providers() -> Vec<String> {
    vec![
        "claude".to_string(),
        "cursor".to_string(),
        "codex".to_string(),
        "opencode".to_string(),
    ]
}

/// Per-provider config sub-tables. Each provider has its own struct because
/// the fields diverge (Cursor has `cli`, Codex has `provider_name`, …).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentProvidersConfig {
    pub claude: AgentProviderConfig,
    pub cursor: CursorProviderConfig,
    pub codex: CodexProviderConfig,
    pub opencode: AgentProviderConfig,
}

/// Generic provider sub-table (Claude, OpenCode, future Gemini).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentProviderConfig {
    /// Model override; empty = vendor default.
    #[serde(default)]
    pub model: String,
    /// Custom OpenAI/Anthropic-compatible base URL; empty = vendor default.
    #[serde(default)]
    pub base_url: String,
    /// Extra CLI flags passed to the provider binary. Validated against a
    /// deny-list of Maestro-owned flags (see [`DENIED_EXTRA_ARG_FLAGS`]).
    #[serde(default)]
    pub extra_args: Vec<String>,
    /// `true` lets users without a personal credential fall back to the
    /// deployment-default env-var token. Default OFF on fresh installs
    /// (04_architecture.md §0 D6).
    #[serde(default)]
    pub allow_shared_default: bool,
}

/// Cursor provider sub-table — diverges from the generic shape because it
/// carries a CLI binary name and **no** `base_url` (amendment A1: Cursor's
/// CLI does not support custom endpoints).
///
/// All fields default to **empty / false** (not the runtime defaults like
/// `"agent"` and `"Auto"`). The "empty" sentinel is meaningful: at load time
/// `migrate_legacy_flat_fields` only copies the legacy `[agent].cursor_cli`
/// into the sub-table when this field is empty, and `effective_cursor_cli`
/// falls back to the legacy field's `default_cursor_cli()` when the
/// sub-table is empty. This lets us distinguish "operator did not configure
/// the sub-table at all" from "operator explicitly set `cli = \"\"`" (the
/// latter is a config error caught by `validate`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CursorProviderConfig {
    #[serde(default)]
    pub cli: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub extra_args: Vec<String>,
    #[serde(default)]
    pub allow_shared_default: bool,
    /// Phase 1 (06_qa_and_blind_spots.md §A.4 T-CFG-002, amendment A1): Cursor's
    /// CLI does not support custom endpoints. The field is accepted (so legacy
    /// configs parse) but `Config::validate()` refuses any non-empty value with
    /// a stable, user-visible error so the operator removes it.
    #[serde(default)]
    pub base_url: String,
}

/// Codex provider sub-table — adds `provider_name` (the named entry in
/// `~/.codex/config.toml [model_providers]`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CodexProviderConfig {
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub provider_name: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub extra_args: Vec<String>,
    #[serde(default)]
    pub allow_shared_default: bool,
}

/// Flags Maestro owns and clients must not override via `extra_args`.
/// Source: 04_architecture.md §0 D10. Adding to this list is a breaking change
/// for any operator who set the flag in their config and must be coordinated
/// with the release notes.
pub const DENIED_EXTRA_ARG_FLAGS: &[&str] = &[
    "--dangerously-skip-permissions",
    "--output-format",
    "--resume",
    "--print",
    "--verbose",
    "-p",
];

/// Return `Err` when `args` contains any flag from [`DENIED_EXTRA_ARG_FLAGS`].
/// Matches whole tokens; `--max-turns` is fine even though `--print` is denied.
pub fn validate_extra_args(args: &[String]) -> Result<()> {
    for a in args {
        let tok = a.trim();
        if DENIED_EXTRA_ARG_FLAGS.contains(&tok) {
            return Err(ConfigError::Validation {
                section: "agent.providers.*",
                field: "extra_args",
                detail: format!(
                    "extra_args_denied: flag \"{tok}\" is reserved by Maestro and cannot be set via extra_args"
                ),
            }
            .into());
        }
    }
    Ok(())
}

fn default_improve_timeout() -> u64 {
    300
}

pub(super) fn default_step_timeout() -> u64 {
    1800
}

/// Normalized value for Cursor Agent `--model`.
///
/// Empty strings and `"auto"` (ASCII case-insensitive) become `"Auto"`. Cursor’s CLI does not treat
/// omitted `--model` the same as Auto in all cases; we always pass `--model` with this value.
pub fn cursor_model_for_cli(model: &str) -> &str {
    let t = model.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("auto") {
        "Auto"
    } else {
        t
    }
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            provider: AiAgentProvider::default(),
            cursor_cli: super::agent_legacy::default_cursor_cli(),
            cursor_model: super::agent_legacy::default_cursor_model(),
            step_timeout_secs: default_step_timeout(),
            improve_timeout_secs: default_improve_timeout(),
            model: String::new(),
            providers: AgentProvidersConfig::default(),
            available_providers: default_available_providers(),
            share_conversation_across_steps: default_share_conversation(),
        }
    }
}

impl AgentConfig {
    /// Return the effective Claude model name, resolving in precedence
    /// order:
    /// 1. `[agent.providers.claude].model` (canonical location, written
    ///    by `PUT /api/config/agent`).
    /// 2. `[agent].model` (legacy flat field — kept one release for
    ///    back-compat; populated by migration of old `config.toml`).
    /// 3. `None` — let `claude` choose its own default model.
    ///
    /// Returning `Option` (not `&str` with a hardcoded fallback) is
    /// deliberate: when both fields are empty/blank the caller MUST omit
    /// the `--model` arg entirely, otherwise pantheon-style proxies that
    /// don't support older opus-4-6/4-7 reject the request. Unlike
    /// Cursor (where the CLI requires `--model`), Claude is happy to run
    /// without one and pick its own current default.
    pub fn effective_claude_model(&self) -> Option<&str> {
        let sub = self.providers.claude.model.trim();
        if !sub.is_empty() {
            return Some(&self.providers.claude.model);
        }
        let legacy = self.model.trim();
        if !legacy.is_empty() {
            return Some(&self.model);
        }
        None
    }

    /// Return the effective OpenCode model name from `[agent.providers.opencode].model`.
    ///
    /// OpenCode has no legacy flat field; resolution is single-source. The
    /// init-shim that materialises `opencode.json` inside the worker uses
    /// this as the `models.<id>` key. Empty / whitespace-only → `None`,
    /// which the validator rejects when `provider = "opencode"` (the model
    /// name is non-optional for self-hosted endpoints — OpenCode needs to
    /// know which model id to call on the OpenAI-compat server).
    pub fn effective_opencode_model(&self) -> Option<&str> {
        let sub = self.providers.opencode.model.trim();
        if sub.is_empty() {
            None
        } else {
            Some(&self.providers.opencode.model)
        }
    }
}

fn default_agent_step_repeat() -> u8 {
    1
}

/// A skill reference in a workflow step — resolved at runtime into a `--system-prompt` (Claude)
/// or a `/skill args` invocation (Cursor).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillRef {
    pub name: String,
    #[serde(default)]
    pub args: Vec<String>,
}

/// Controls when an agent step is eligible to run based on ticketing system availability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StepAvailability {
    /// Run regardless of ticketing system status (default when omitted).
    #[default]
    Always,
    /// Run only when a ticketing system (`jira` or `github`) is active.
    Ticketing,
    /// Run only when **no** ticketing system is active.
    NoTicketing,
}

/// One step in the ticket workflow (`[[agent_steps]]` in TOML).
///
/// A step is either an **agent step** (has `prompt` and/or `skills`) or a **command step**
/// (has `commands`). The two modes are mutually exclusive.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentStepConfig {
    pub name: String,
    /// Prompt sent to the AI agent. Mutually exclusive with `commands`.
    #[serde(default)]
    pub prompt: String,
    /// Run this step this many times in sequence (each run after the first uses `--resume`
    /// for agent steps, or re-runs the full command list for command steps). Default `1`.
    #[serde(default = "default_agent_step_repeat")]
    pub repeat: u8,
    /// Optional skills to load for this step (agent steps only).
    #[serde(default)]
    pub skills: Vec<SkillRef>,
    /// Resume the previous step's Claude Code session instead of starting fresh.
    /// When `true`, the step continues with full conversation history from the prior step.
    /// Default `false` — each step gets a clean session. Ignored on command steps.
    #[serde(default)]
    pub resume_previous: bool,
    /// When this step is eligible to run: `"always"` (default), `"ticketing"` (only when a ticketing
    /// system is active), or `"no_ticketing"` (only when no ticketing system is active).
    #[serde(default)]
    pub when: StepAvailability,
    /// Shell commands to execute sequentially. Mutually exclusive with `prompt` and `skills`.
    /// When present, the step runs each command via `bash -c` in the worktree directory
    /// instead of launching an AI agent session.
    #[serde(default)]
    pub commands: Vec<String>,
}

impl AgentStepConfig {
    /// Returns `true` if this step should run given the current ticketing system availability.
    pub fn available_for(&self, ticketing_available: bool) -> bool {
        match self.when {
            StepAvailability::Always => true,
            StepAvailability::Ticketing => ticketing_available,
            StepAvailability::NoTicketing => !ticketing_available,
        }
    }

    /// Returns `true` if this step executes shell commands instead of an AI agent session.
    pub fn is_command_step(&self) -> bool {
        !self.commands.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ai_agent_provider_parse_and_as_str_round_trip() {
        for name in ["claude", "cursor", "codex", "opencode"] {
            let p = AiAgentProvider::parse(name).unwrap();
            assert_eq!(p.as_str(), name);
        }
        assert!(AiAgentProvider::parse("gemini").is_err());
        assert!(AiAgentProvider::parse("").is_err());
    }

    #[test]
    fn ai_agent_provider_runtime_implemented_for_all_four_v1_providers() {
        // Phase 4 wired codex + opencode adapters alongside claude + cursor.
        assert!(AiAgentProvider::Claude.is_runtime_implemented());
        assert!(AiAgentProvider::Cursor.is_runtime_implemented());
        assert!(AiAgentProvider::Codex.is_runtime_implemented());
        assert!(AiAgentProvider::OpenCode.is_runtime_implemented());
    }

    #[test]
    fn validate_extra_args_accepts_user_flags() {
        validate_extra_args(&["--max-turns".into(), "50".into()]).unwrap();
        validate_extra_args(&[]).unwrap();
        validate_extra_args(&["--something-custom".into()]).unwrap();
    }

    #[test]
    fn validate_extra_args_rejects_denied_flags() {
        for denied in [
            "--dangerously-skip-permissions",
            "--output-format",
            "--resume",
            "--print",
            "--verbose",
            "-p",
        ] {
            let err = validate_extra_args(&[denied.into()])
                .expect_err(&format!("flag {denied} must be rejected"));
            let msg = err.to_string();
            assert!(
                msg.contains("extra_args_denied"),
                "error message should carry the stable code 'extra_args_denied', got: {msg}"
            );
            assert!(
                msg.contains(denied),
                "error message should name the denied flag, got: {msg}"
            );
        }
    }

    #[test]
    fn cursor_model_for_cli_normalizes_auto_and_empty() {
        assert_eq!(cursor_model_for_cli(""), "Auto");
        assert_eq!(cursor_model_for_cli("   "), "Auto");
        assert_eq!(cursor_model_for_cli("Auto"), "Auto");
        assert_eq!(cursor_model_for_cli("auto"), "Auto");
        assert_eq!(cursor_model_for_cli("AUTO"), "Auto");
    }

    #[test]
    fn cursor_model_for_cli_passes_concrete_name() {
        assert_eq!(cursor_model_for_cli("gpt-4.1"), "gpt-4.1");
        assert_eq!(cursor_model_for_cli("  sonnet  "), "sonnet");
    }

    // -- Command step tests --

    #[test]
    fn is_command_step_true_when_commands_present() {
        let step = AgentStepConfig {
            name: "Run tests".into(),
            prompt: String::new(),
            repeat: 1,
            skills: Vec::new(),
            resume_previous: false,
            when: StepAvailability::Always,
            commands: vec!["npm test".into()],
        };
        assert!(step.is_command_step());
    }

    #[test]
    fn is_command_step_false_when_no_commands() {
        let step = AgentStepConfig {
            name: "Implement".into(),
            prompt: "do stuff".into(),
            repeat: 1,
            skills: Vec::new(),
            resume_previous: false,
            when: StepAvailability::Always,
            commands: Vec::new(),
        };
        assert!(!step.is_command_step());
    }

    // ─── effective_claude_model precedence ──────────────────────────────

    fn agent_cfg(legacy: &str, sub: &str) -> AgentConfig {
        let mut cfg = AgentConfig {
            model: legacy.to_string(),
            ..AgentConfig::default()
        };
        cfg.providers.claude.model = sub.to_string();
        cfg
    }

    /// T-MODEL-RESOLVE-001: sub-table set, legacy set → sub-table wins.
    #[test]
    fn effective_claude_model_subtable_wins_over_legacy() {
        let cfg = agent_cfg("legacy-old-model", "sub-new-model");
        assert_eq!(cfg.effective_claude_model(), Some("sub-new-model"));
    }

    /// T-MODEL-RESOLVE-002: sub-table empty, legacy set → legacy used
    /// (one-release back-compat for users still on the migrated layout).
    #[test]
    fn effective_claude_model_falls_back_to_legacy_when_subtable_empty() {
        let cfg = agent_cfg("legacy-model", "");
        assert_eq!(cfg.effective_claude_model(), Some("legacy-model"));
    }

    /// T-MODEL-RESOLVE-003: sub-table set, legacy empty → sub-table used.
    #[test]
    fn effective_claude_model_subtable_used_when_legacy_empty() {
        let cfg = agent_cfg("", "sub-model");
        assert_eq!(cfg.effective_claude_model(), Some("sub-model"));
    }

    /// T-MODEL-RESOLVE-004: both empty → None (omit `--model` arg). This
    /// is the actual task #44 fix surface — pantheon-style proxies that
    /// don't recognise the older migrated models need `--model` omitted
    /// so claude picks a default the proxy DOES support.
    #[test]
    fn effective_claude_model_returns_none_when_both_empty() {
        let cfg = agent_cfg("", "");
        assert_eq!(cfg.effective_claude_model(), None);
    }

    /// T-MODEL-RESOLVE-005: sub-table whitespace-only → treated as empty.
    /// Matches the trim semantics of the existing `effective_cursor_*`
    /// helpers so a user pasting "   " into the dashboard input still
    /// resolves to None / legacy.
    #[test]
    fn effective_claude_model_treats_whitespace_subtable_as_empty() {
        let cfg = agent_cfg("legacy", "   ");
        assert_eq!(
            cfg.effective_claude_model(),
            Some("legacy"),
            "whitespace-only sub-table must fall through to legacy"
        );

        let cfg = agent_cfg("   ", "   ");
        assert_eq!(
            cfg.effective_claude_model(),
            None,
            "all-whitespace must resolve to None"
        );
    }

    /// effective_opencode_model: set → Some.
    #[test]
    fn effective_opencode_model_returns_set_value() {
        let mut cfg = AgentConfig::default();
        cfg.providers.opencode.model = "lmstudio/qwen3-coder".into();
        assert_eq!(cfg.effective_opencode_model(), Some("lmstudio/qwen3-coder"));
    }

    /// effective_opencode_model: empty / whitespace → None. Validator
    /// catches this when `provider = "opencode"` so the shim never sees
    /// the None case — but defence in depth.
    #[test]
    fn effective_opencode_model_returns_none_when_empty_or_whitespace() {
        let mut cfg = AgentConfig::default();
        assert_eq!(cfg.effective_opencode_model(), None);
        cfg.providers.opencode.model = "   ".into();
        assert_eq!(cfg.effective_opencode_model(), None);
    }
}
