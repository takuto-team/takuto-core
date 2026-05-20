// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{MaestroError, Result};

/// Which CLI implements ticket implementation / review / fix steps.
///
/// Phase 1 (04_architecture.md §0 D1, A1, A2): four native adapters in v1.
/// `Claude` and `Cursor` are wired today; `Codex` and `OpenCode` are enum
/// placeholders only — workflow execution against them fails with a clear
/// "not yet implemented (Phase 4)" error.
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
            other => Err(MaestroError::Config(format!(
                "unknown agent provider \"{other}\" (expected one of: claude, cursor, codex, opencode)"
            ))),
        }
    }

    /// `true` for providers whose runtime adapter is wired in v1. `Codex` and
    /// `OpenCode` are config-only until Phase 4.
    pub fn is_runtime_implemented(self) -> bool {
        matches!(self, AiAgentProvider::Claude | AiAgentProvider::Cursor)
    }
}

/// Which ticketing system (if any) drives workflow automation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum TicketingSystem {
    /// No ticketing integration — manual description entry only (default).
    #[default]
    None,
    /// Jira via `acli` — current behavior with auto-polling and ticket transitions.
    Jira,
    /// GitHub Issues — poll open issues, no Atlassian auth required.
    GitHub,
}

impl fmt::Display for TicketingSystem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("none"),
            Self::Jira => f.write_str("jira"),
            Self::GitHub => f.write_str("github"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    #[serde(default)]
    pub provider: AiAgentProvider,
    /// **Deprecated (Phase 1)**: legacy top-level Cursor CLI binary. Moved to
    /// `[agent.providers.cursor].cli`. Still parsed for one release; migrated
    /// at load time and overwritten on next save (see `Config::load`).
    #[serde(default = "default_cursor_cli")]
    pub cursor_cli: String,
    /// **Deprecated (Phase 1)**: legacy top-level Cursor model. Moved to
    /// `[agent.providers.cursor].model`. Migrated at load time.
    #[serde(default = "default_cursor_model")]
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
}

fn default_available_providers() -> Vec<String> {
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
            return Err(MaestroError::Config(format!(
                "extra_args_denied: flag \"{tok}\" is reserved by Maestro and cannot be set via [agent.providers.*].extra_args"
            )));
        }
    }
    Ok(())
}

fn default_improve_timeout() -> u64 {
    300
}

fn default_cursor_cli() -> String {
    "agent".to_string()
}

fn default_cursor_model() -> String {
    "Auto".to_string()
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
            cursor_cli: default_cursor_cli(),
            cursor_model: default_cursor_model(),
            step_timeout_secs: default_step_timeout(),
            improve_timeout_secs: default_improve_timeout(),
            model: String::new(),
            providers: AgentProvidersConfig::default(),
            available_providers: default_available_providers(),
        }
    }
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

    /// Task #44: return the effective Claude model name, resolving in
    /// precedence order:
    /// 1. `[agent.providers.claude].model` (Phase 1 sub-table — the
    ///    canonical location, written by `PUT /api/config/agent`).
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

pub fn interpolate_agent_prompt(template: &str, vars: &HashMap<String, String>) -> String {
    interpolate_template(template, vars, false)
}

/// Like [`interpolate_agent_prompt`], but wraps each substituted value in
/// single-quotes so it is safe to embed in a `bash -c` command string.
///
/// Use this for **command steps** where the interpolated result is executed as
/// a shell command and the variable values may contain untrusted content
/// (e.g. ticket titles from GitHub issues).
pub fn interpolate_command_template(template: &str, vars: &HashMap<String, String>) -> String {
    interpolate_template(template, vars, true)
}

/// Shell-escape a string by wrapping it in single quotes.
/// Any embedded single quotes are replaced with `'\''` (end quote, escaped
/// literal, restart quote).
fn shell_escape_value(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn interpolate_template(
    template: &str,
    vars: &HashMap<String, String>,
    shell_escape: bool,
) -> String {
    let mut out = String::with_capacity(template.len() + 64);
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        if rest.starts_with("{{") {
            out.push('{');
            rest = &rest[2..];
            continue;
        }
        let Some(end_rel) = rest.find('}') else {
            out.push_str(rest);
            return out;
        };
        let key = &rest[1..end_rel];
        if let Some(val) = vars.get(key) {
            if shell_escape {
                out.push_str(&shell_escape_value(val));
            } else {
                out.push_str(val);
            }
        } else {
            out.push_str(&rest[..=end_rel]);
        }
        rest = &rest[end_rel + 1..];
    }
    out.push_str(rest);
    out
}

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

/// Task #47: tool-provisioning block. List of shell commands that run as
/// **root** in the maestro container at startup (before `setpriv` to the
/// `maestro` user) to populate the shared `maestro-tools` Docker volume
/// at `/opt/maestro-tools/bin`. The volume is bind-mounted **read-only**
/// into every spawned worker / editor / run-command via
/// `container.rs::base_docker_args`, so anything installed here is
/// available to claude / cursor / scripts on `$PATH`.
///
/// **SHA-gated**: the canonical sha256 of the (sorted, JSON-encoded)
/// `install_commands` list is written to
/// `/opt/maestro-tools/.provisioning-sha` on full success. Subsequent
/// boots compare the live config's SHA against the file; if they match
/// the install pass is skipped (fast path). Edit the list (add, remove,
/// reorder, or tweak a command) → SHA changes → install pass runs again.
///
/// **Per-command idempotency** is the admin's responsibility — guard
/// each command with `[ -f "$MAESTRO_TOOLS_BIN/<name>" ] || …` (matching
/// the defaults shipped in `config.toml.example`) so re-runs after
/// adding an unrelated tool don't re-fetch the unchanged ones.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProvisioningConfig {
    /// One shell command per element. Each runs via `bash -c "$cmd"` as
    /// root, with `MAESTRO_TOOLS_BIN=/opt/maestro-tools/bin` exported.
    /// Empty list (the default) → no-op fast path; the install pass
    /// records its empty-list SHA and skips on subsequent boots.
    #[serde(default)]
    pub install_commands: Vec<String>,
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

/// Dev-only knobs. Off by default in production. Never read inside any code path
/// that runs against real users without an explicit `[dev]` opt-in.
///
/// See `crates/maestro-core/src/dev_mock.rs` for the mock-agent behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevConfig {
    /// When `true`, `ClaudeSession::run_prompt` and `CursorSession::run_prompt`
    /// short-circuit to a scripted mock session. **No Claude/Cursor process is spawned.**
    /// Honors the env override `MAESTRO_DEV_MOCK_AGENT=1`.
    #[serde(default)]
    pub mock_agent: bool,

    /// Optional path to a text file used as the mock's line script (one emit per line).
    /// Relative paths resolve against the config file directory.
    /// When `None` (default), the built-in `DEFAULT_MOCK_SCRIPT` is used.
    #[serde(default)]
    pub mock_agent_script_path: Option<String>,

    /// Delay between emitted lines in ms. Default 75.
    #[serde(default = "default_mock_line_delay_ms")]
    pub mock_agent_line_delay_ms: u64,

    /// Total mock session duration cap in ms. The mock will stop emitting after this
    /// even if the script has more lines. Default 5000.
    #[serde(default = "default_mock_total_ms")]
    pub mock_agent_total_ms: u64,
}

impl Default for DevConfig {
    fn default() -> Self {
        Self {
            mock_agent: false,
            mock_agent_script_path: None,
            mock_agent_line_delay_ms: default_mock_line_delay_ms(),
            mock_agent_total_ms: default_mock_total_ms(),
        }
    }
}

fn default_mock_line_delay_ms() -> u64 {
    75
}
fn default_mock_total_ms() -> u64 {
    5000
}

/// Docker-specific hooks (see README). `build_commands` run at image build time; `compose_up_commands` on each container start.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DockerConfig {
    /// Shell commands (`bash -c`) executed once while building the image, after tools are installed.
    #[serde(default)]
    pub build_commands: Vec<String>,
    /// Shell commands executed on every `docker compose up` as the maestro user, after auth preflight, before the server.
    #[serde(default)]
    pub compose_up_commands: Vec<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralConfig {
    #[serde(default)]
    pub dry_mode: bool,
    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,
    /// When **`true`** (default), Jira polling starts automatically on startup. Set to **`false`** to start in the same **paused** state as **Pause polling** on the dashboard (no **`poll_once`** until **Resume polling** or **`POST /api/polling/resume`**). Not persisted when toggled at runtime; restart re-reads this flag from **`config.toml`**.
    #[serde(default = "default_auto_polling")]
    pub auto_polling: bool,
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent_workflows: u32,
    /// Max **visible** workflows on the dashboard (rows still in the map: **Done**, paused, stopped, error, in-progress all count). `0` means use **`max_concurrent_workflows`**.
    #[serde(default)]
    pub max_active_workflows: u32,
    /// Max **manual** dashboard-started ticket workflows that still **occupy a slot** (not **Done**, **Stopped**, or **Error**). `0` means no limit.
    #[serde(default)]
    pub max_concurrent_manual_workflows: u32,
    #[serde(default = "default_log_level")]
    pub log_level: String,
    /// Docker image for workflow worker containers. Empty = auto-detect from running Maestro container.
    #[serde(default)]
    pub worker_image: String,
    /// Which ticketing system drives workflow automation. Default `none` (no ticketing integration).
    #[serde(default)]
    pub ticketing_system: TicketingSystem,
    /// Interval in seconds for polling PR merge status via the GitHub API (`0` disables polling). Default: 60.
    #[serde(default = "default_pr_merge_poll_interval")]
    pub pr_merge_poll_interval_secs: u64,
    /// When `true`, each agent step prompt includes instructions to append findings to
    /// `lore/reports/<item-key>_report.md` and a final consolidation step produces a polished
    /// summary after all custom steps complete. Default `false`.
    #[serde(default)]
    pub generate_report: bool,
    /// Directory containing dynamic workflow definition YAML files. Relative to the config file
    /// directory, or absolute. Default `"workflows"`.
    #[serde(default = "default_workflow_definitions_dir")]
    pub workflow_definitions_dir: String,
    /// Username of the user who owns workflows created automatically by the Jira/GitHub poller.
    /// When `None` (default), the poller falls back to the lexicographically-first non-suspended
    /// admin. When set but the named user is missing or suspended, a warning is logged and the
    /// fallback is used. When neither resolves, polling-created workflows are skipped entirely.
    #[serde(default)]
    pub poller_owner_username: Option<String>,
    /// When `true`, workflows restored from snapshot with `user_id == None` (e.g. pre-multi-user
    /// orphans) are reassigned to the resolved poller owner at startup so they appear on that
    /// user's dashboard. Default `false` — orphan workflows remain invisible until an explicit
    /// migration is requested.
    #[serde(default)]
    pub migrate_orphan_workflows: bool,
    /// Plan-10: when `true` (default), startup reconciliation back-fills
    /// `user_repositories` rows from restored snapshot workflows — every
    /// workflow whose `user_id` is set and whose `workspace_name` matches a
    /// registered repository's name gets a `(user_id, repository_id)`
    /// association created so the dashboard list filter (Step 6) shows the
    /// workflow to its owner. Set to `false` if the operator wants
    /// pre-existing workflows to STAY hidden on their owner's dashboard until
    /// the owner explicitly adds the repository.
    #[serde(default = "default_migrate_orphan_repo_associations")]
    pub migrate_orphan_repo_associations: bool,
    /// Phase 2a (04_architecture.md §3.2): controls whether the server will
    /// auto-generate `${data_dir}/secret.key` on first boot when neither
    /// `MAESTRO_SECRET_KEY` nor an existing keyfile is present. Default
    /// **`true`** so single-tenant + fresh installs Just Work. Set to `false`
    /// in hardened environments where the operator wants to provision the
    /// key out of band; the server then boots in degraded mode until the
    /// keyfile or env var is provided.
    #[serde(default = "default_allow_auto_generate_secret_key")]
    pub allow_auto_generate_secret_key: bool,
}

fn default_migrate_orphan_repo_associations() -> bool {
    true
}

fn default_allow_auto_generate_secret_key() -> bool {
    true
}

fn default_workflow_definitions_dir() -> String {
    "workflows".to_string()
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
pub struct GitConfig {
    #[serde(default = "default_base_branch")]
    pub base_branch: String,
    /// Git remote name for fetch, worktree base ref, and push (default `origin`).
    #[serde(default = "default_git_remote")]
    pub remote: String,
    #[serde(default = "default_repo_path")]
    pub repo_path: String,
}

/// GitHub App credentials for bot-attributed commits and pull requests.
///
/// When all required fields are set, Maestro authenticates as the GitHub App's
/// bot identity instead of the personal `gh` user. Commits and PRs will be
/// attributed to `maestro-bot[bot]`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GitHubAppConfig {
    /// The GitHub App's numeric App ID.
    #[serde(default)]
    pub app_id: u64,
    /// The installation ID for the target org/repository.
    #[serde(default)]
    pub app_installation_id: u64,
    /// Display name of the GitHub App (e.g. `"sous-coder"`). Shown in the dashboard header.
    #[serde(default)]
    pub app_name: String,
    /// PEM-encoded RSA private key for signing JWTs (inline content).
    /// Set **either** this or `app_private_key_path`, not both.
    #[serde(default)]
    pub app_private_key: String,
    /// Path to a PEM-encoded RSA private key file.
    /// Set **either** this or `app_private_key`, not both.
    #[serde(default)]
    pub app_private_key_path: String,
}

impl GitHubAppConfig {
    /// Returns `true` when the minimum required fields are set (app_id, installation_id,
    /// and at least one private key source).
    pub fn is_configured(&self) -> bool {
        self.app_id != 0
            && self.app_installation_id != 0
            && (!self.app_private_key.trim().is_empty()
                || !self.app_private_key_path.trim().is_empty())
    }
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

fn default_poll_interval() -> u64 {
    60
}
fn default_auto_polling() -> bool {
    true
}
fn default_max_concurrent() -> u32 {
    1
}
fn default_log_level() -> String {
    "info".to_string()
}
fn default_item_types() -> Vec<String> {
    vec!["Task".to_string(), "Bug".to_string()]
}
fn default_base_branch() -> String {
    "main".to_string()
}
fn default_repo_path() -> String {
    "/workspace".to_string()
}
fn default_git_remote() -> String {
    "origin".to_string()
}
fn default_host() -> String {
    "0.0.0.0".to_string()
}
fn default_port() -> u16 {
    8080
}
fn default_step_timeout() -> u64 {
    1800
}
fn default_pr_merge_poll_interval() -> u64 {
    60
}

impl GeneralConfig {
    /// Effective cap on **visible** dashboard workflows for the Jira poller. **`max_active_workflows == 0`** mirrors **`max_concurrent_workflows`**.
    pub fn effective_max_active_workflows(&self) -> u32 {
        if self.max_active_workflows == 0 {
            self.max_concurrent_workflows
        } else {
            self.max_active_workflows
        }
    }
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            dry_mode: false,
            poll_interval_secs: default_poll_interval(),
            auto_polling: true,
            max_concurrent_workflows: default_max_concurrent(),
            max_active_workflows: 0,
            max_concurrent_manual_workflows: 0,
            log_level: default_log_level(),
            worker_image: String::new(),
            ticketing_system: TicketingSystem::None,
            pr_merge_poll_interval_secs: default_pr_merge_poll_interval(),
            generate_report: false,
            workflow_definitions_dir: default_workflow_definitions_dir(),
            poller_owner_username: None,
            migrate_orphan_workflows: false,
            migrate_orphan_repo_associations: default_migrate_orphan_repo_associations(),
            allow_auto_generate_secret_key: default_allow_auto_generate_secret_key(),
        }
    }
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

impl Default for GitConfig {
    fn default() -> Self {
        Self {
            base_branch: default_base_branch(),
            remote: default_git_remote(),
            repo_path: default_repo_path(),
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
    fn interpolate_agent_prompt_substitutes_placeholders() {
        let mut vars = HashMap::new();
        vars.insert("ticket_key".into(), "PROJ-1".into());
        vars.insert("ticket_summary".into(), "Fix login".into());
        assert_eq!(
            interpolate_agent_prompt("{ticket_key}: {ticket_summary}", &vars),
            "PROJ-1: Fix login"
        );
    }

    #[test]
    fn interpolate_agent_prompt_leaves_unknown_braces() {
        let vars = HashMap::new();
        assert_eq!(
            interpolate_agent_prompt("x {unknown} y", &vars),
            "x {unknown} y"
        );
    }

    #[test]
    fn interpolate_command_template_shell_escapes_values() {
        let mut vars = HashMap::new();
        vars.insert("ticket_key".into(), "GH-1".into());
        vars.insert("ticket_summary".into(), "Fix $(rm -rf /) bug".into());
        assert_eq!(
            interpolate_command_template("echo {ticket_key} {ticket_summary}", &vars),
            "echo 'GH-1' 'Fix $(rm -rf /) bug'"
        );
    }

    #[test]
    fn interpolate_command_template_escapes_single_quotes() {
        let mut vars = HashMap::new();
        vars.insert("val".into(), "it's broken".into());
        assert_eq!(
            interpolate_command_template("echo {val}", &vars),
            "echo 'it'\\''s broken'"
        );
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

    // -- GitHubAppConfig::is_configured --

    #[test]
    fn github_app_config_unconfigured_by_default() {
        assert!(!GitHubAppConfig::default().is_configured());
    }

    #[test]
    fn github_app_config_requires_app_id() {
        let cfg = GitHubAppConfig {
            app_id: 0,
            app_installation_id: 42,
            app_private_key: "pem".into(),
            ..Default::default()
        };
        assert!(!cfg.is_configured());
    }

    #[test]
    fn github_app_config_requires_installation_id() {
        let cfg = GitHubAppConfig {
            app_id: 99,
            app_installation_id: 0,
            app_private_key: "pem".into(),
            ..Default::default()
        };
        assert!(!cfg.is_configured());
    }

    #[test]
    fn github_app_config_requires_private_key_source() {
        let cfg = GitHubAppConfig {
            app_id: 99,
            app_installation_id: 42,
            app_private_key: "   ".into(),
            app_private_key_path: "   ".into(),
            ..Default::default()
        };
        assert!(!cfg.is_configured());
    }

    #[test]
    fn github_app_config_configured_with_inline_key() {
        let cfg = GitHubAppConfig {
            app_id: 99,
            app_installation_id: 42,
            app_private_key: "-----BEGIN RSA PRIVATE KEY-----".into(),
            ..Default::default()
        };
        assert!(cfg.is_configured());
    }

    #[test]
    fn github_app_config_configured_with_key_path() {
        let cfg = GitHubAppConfig {
            app_id: 99,
            app_installation_id: 42,
            app_private_key_path: "/etc/maestro/key.pem".into(),
            ..Default::default()
        };
        assert!(cfg.is_configured());
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

    // -- effective_max_active_workflows --

    #[test]
    fn effective_max_active_workflows_returns_max_active_when_nonzero() {
        let general = GeneralConfig {
            max_active_workflows: 5,
            max_concurrent_workflows: 3,
            ..Default::default()
        };
        assert_eq!(general.effective_max_active_workflows(), 5);
    }

    #[test]
    fn effective_max_active_workflows_falls_back_to_concurrent_when_zero() {
        let general = GeneralConfig {
            max_active_workflows: 0,
            max_concurrent_workflows: 4,
            ..Default::default()
        };
        assert_eq!(general.effective_max_active_workflows(), 4);
    }

    // ─── Phase 1: provider sub-tables, migration, validation ─────────────

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
    fn ai_agent_provider_runtime_implemented_only_for_claude_cursor() {
        assert!(AiAgentProvider::Claude.is_runtime_implemented());
        assert!(AiAgentProvider::Cursor.is_runtime_implemented());
        assert!(!AiAgentProvider::Codex.is_runtime_implemented());
        assert!(!AiAgentProvider::OpenCode.is_runtime_implemented());
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

    // ─── Task #44: effective_claude_model precedence ────────────────────

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
