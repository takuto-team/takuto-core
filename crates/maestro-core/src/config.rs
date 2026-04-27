// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::de::{self, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use tracing::warn;

use crate::error::{MaestroError, Result};

/// Which CLI implements ticket implementation / review / fix steps.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum AiAgentProvider {
    #[default]
    Claude,
    Cursor,
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
    /// Cursor Agent CLI executable (install script usually provides `agent` on PATH).
    #[serde(default = "default_cursor_cli")]
    pub cursor_cli: String,
    /// Cursor Agent `--model`. Default `"Auto"` requests Cursor automatic model selection.
    #[serde(default = "default_cursor_model")]
    pub cursor_model: String,
    /// Timeout per agent session (applies to all providers).
    #[serde(default = "default_step_timeout")]
    pub step_timeout_secs: u64,
    /// Timeout in seconds for "Improve with AI" / "Prompt ticket" sessions. Default 300.
    #[serde(default = "default_improve_timeout")]
    pub improve_timeout_secs: u64,
    /// Model override (e.g. `"claude-opus-4-6"`). Empty = provider default.
    #[serde(default)]
    pub model: String,
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
        }
    }
}

fn default_agent_step_repeat() -> u8 {
    1
}

/// Ticket pipeline steps loaded from a standalone TOML file (`[[agent_steps]]` only).
#[derive(Debug, Deserialize)]
struct TicketWorkflowStepsFile {
    #[serde(default)]
    agent_steps: Vec<AgentStepConfig>,
}

/// PR review steps loaded from a standalone TOML file (`[[review_agent_steps]]` only).
#[derive(Debug, Deserialize)]
struct ReviewWorkflowStepsFile {
    #[serde(default)]
    review_agent_steps: Vec<AgentStepConfig>,
}

/// Merge-base steps loaded from a standalone TOML file (`[[merge_base_agent_steps]]` only).
#[derive(Debug, Deserialize)]
struct MergeBaseWorkflowStepsFile {
    #[serde(default)]
    merge_base_agent_steps: Vec<AgentStepConfig>,
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

/// Built-in agent steps when `[[agent_steps]]` is omitted or empty (generic prompts, no slash-commands).
pub fn default_agent_steps() -> Vec<AgentStepConfig> {
    vec![
        AgentStepConfig {
            name: "Implement ticket".to_string(),
            prompt: "Implement this Jira ticket following the project's conventions.\n\nTicket context:\n{ticket_context}\n\nAdd or update tests as appropriate.".to_string(),
            repeat: 1,
            skills: Vec::new(),
            resume_previous: false,
            when: StepAvailability::Always,
            commands: Vec::new(),
        },
        AgentStepConfig {
            name: "Review changes".to_string(),
            prompt: "Review all uncommitted changes for correctness, security, and code style. Fix any issues."
                .to_string(),
            repeat: 1,
            skills: Vec::new(),
            resume_previous: false,
            when: StepAvailability::Always,
            commands: Vec::new(),
        },
    ]
}

/// Built-in steps for **`[[merge_base_agent_steps]]`** when that list is empty.
pub fn default_merge_base_agent_steps() -> Vec<AgentStepConfig> {
    vec![AgentStepConfig {
        name: "Merge base branch".to_string(),
        prompt: "Fetch the latest base branch (`{base_branch}`) from remote and merge it into the current branch.\n\
            Resolve any conflicts — prefer the current branch's changes for new code, but keep the base branch's \
            changes for configuration and dependency files unless they conflict with this ticket's work.\n\
            Run lint and tests after the merge. Commit the merge and push."
            .to_string(),
        repeat: 1,
        skills: Vec::new(),
        resume_previous: false,
        when: StepAvailability::Always,
        commands: Vec::new(),
    }]
}

/// Built-in steps for **`[[review_agent_steps]]`** when that list is empty (PR comment loop after main flow is Done).
pub fn default_review_agent_steps() -> Vec<AgentStepConfig> {
    vec![AgentStepConfig {
        name: "Address PR feedback".to_string(),
        prompt: "Pull request URL: {pr_url}\n\nTicket context:\n{ticket_context}\n\nUse GitHub tooling (e.g. `gh pr view`, `gh api` for review comments) to find unresolved PR feedback. Address each item with code changes, commit, and push updates to the PR branch."
            .to_string(),
        repeat: 1,
        skills: Vec::new(),
        resume_previous: false,
        when: StepAvailability::Always,
        commands: Vec::new(),
    }]
}

/// Replace `{variable}` placeholders using `vars`. Unknown names are left unchanged.
/// Validate a list of agent/command steps, reporting errors with the given `list_name`
/// (e.g. `"agent_steps"`, `"review_agent_steps"`).
fn validate_step_list(steps: &[AgentStepConfig], list_name: &str) -> Result<()> {
    for step in steps {
        if step.name.trim().is_empty() {
            return Err(MaestroError::Config(format!(
                "Each [[{list_name}]] entry must have a non-empty name"
            )));
        }
        if step.is_command_step() {
            // Command step: reject if prompt or skills are also set.
            if !step.prompt.trim().is_empty() {
                return Err(MaestroError::Config(format!(
                    "Step {:?} in [[{list_name}]]: cannot specify both `commands` and `prompt` (mutually exclusive)",
                    step.name
                )));
            }
            if !step.skills.is_empty() {
                return Err(MaestroError::Config(format!(
                    "Step {:?} in [[{list_name}]]: cannot specify both `commands` and `skills` (skills are agent-only)",
                    step.name
                )));
            }
        } else {
            // Agent step: require prompt or skills.
            if step.prompt.trim().is_empty() && step.skills.is_empty() {
                return Err(MaestroError::Config(format!(
                    "Step {:?} in [[{list_name}]] must have a non-empty `prompt` and/or at least one `skill`",
                    step.name
                )));
            }
        }
        if step.repeat < 1 {
            return Err(MaestroError::Config(format!(
                "Step {:?} in [[{list_name}]]: repeat must be at least 1",
                step.name
            )));
        }
    }
    Ok(())
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
    pub commands: CommandsConfig,
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
    /// User-defined run commands available from the dashboard after a workflow completes.
    /// Each entry defines a named shell command (e.g. dev server, Storybook) that can be
    /// started/stopped from the workflow card.
    #[serde(default)]
    pub run_commands: Vec<RunCommandConfig>,
    /// Ordered AI prompt steps (`[[agent_steps]]`). Empty → [`default_agent_steps`].
    #[serde(default)]
    pub agent_steps: Vec<AgentStepConfig>,
    /// PR review loop (`[[review_agent_steps]]`) after main flow is Done. Empty → [`default_review_agent_steps`].
    #[serde(default)]
    pub review_agent_steps: Vec<AgentStepConfig>,
    /// Merge base branch loop (`[[merge_base_agent_steps]]`) after main flow is Done. Empty → [`default_merge_base_agent_steps`].
    #[serde(default)]
    pub merge_base_agent_steps: Vec<AgentStepConfig>,
}

/// A user-defined run command that can be launched from the dashboard after a workflow completes.
/// Each command runs in a dedicated container with port detection and forwarding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunCommandConfig {
    /// Display label for the command (e.g. `"Dev Server"`, `"Storybook"`).
    pub name: String,
    /// Shell command to execute (e.g. `"npm run dev"`, `"npx storybook dev -p 6006"`).
    pub command: String,
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
    /// Load **`[[agent_steps]]`** from this file (relative to the main config directory, or absolute). Empty = use inline **`agent_steps`** in `config.toml`.
    #[serde(default)]
    pub ticket_workflow_steps_file: String,
    /// Load **`[[review_agent_steps]]`** from this file. Empty = use inline **`review_agent_steps`** in `config.toml`.
    #[serde(default)]
    pub review_workflow_steps_file: String,
    /// Load **`[[merge_base_agent_steps]]`** from this file. Empty = use inline **`merge_base_agent_steps`** in `config.toml`.
    #[serde(default)]
    pub merge_base_workflow_steps_file: String,
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
    #[serde(default)]
    pub repo_url: String,
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

fn deserialize_pre_install_vec<'de, D>(
    deserializer: D,
) -> std::result::Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    struct PreInstallVisitor;

    impl<'de> Visitor<'de> for PreInstallVisitor {
        type Value = Vec<String>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a string or array of strings")
        }

        fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Self::Value, E> {
            let t = v.trim();
            if t.is_empty() {
                Ok(Vec::new())
            } else {
                Ok(vec![t.to_string()])
            }
        }

        fn visit_string<E: de::Error>(self, v: String) -> std::result::Result<Self::Value, E> {
            let t = v.trim();
            if t.is_empty() {
                Ok(Vec::new())
            } else {
                Ok(vec![t.to_string()])
            }
        }

        fn visit_seq<A>(self, mut seq: A) -> std::result::Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let mut out = Vec::new();
            while let Some(s) = seq.next_element::<String>()? {
                let t = s.trim();
                if !t.is_empty() {
                    out.push(t.to_string());
                }
            }
            Ok(out)
        }
    }

    deserializer.deserialize_any(PreInstallVisitor)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CommandsConfig {
    #[serde(default, deserialize_with = "deserialize_pre_install_vec")]
    pub pre_install: Vec<String>,
    #[serde(default)]
    pub install: String,
    /// Shell commands executed before each workflow starts (after worktree creation, before agent steps).
    #[serde(default, deserialize_with = "deserialize_pre_install_vec")]
    pub pre_workflow: Vec<String>,
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
            ticket_workflow_steps_file: String::new(),
            review_workflow_steps_file: String::new(),
            merge_base_workflow_steps_file: String::new(),
            ticketing_system: TicketingSystem::None,
            pr_merge_poll_interval_secs: default_pr_merge_poll_interval(),
            generate_report: false,
            workflow_definitions_dir: default_workflow_definitions_dir(),
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
            repo_url: String::new(),
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
        }
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

impl Config {
    /// After deserializing `config.toml`, replace step lists from optional per-workflow TOML files.
    pub fn apply_workflow_step_files(&mut self, config_file_dir: &Path) -> Result<()> {
        if !self.general.ticket_workflow_steps_file.trim().is_empty() {
            let p = resolve_config_relative_path(
                config_file_dir,
                &self.general.ticket_workflow_steps_file,
            );
            if !p.is_file() {
                warn!(
                    path = %p.display(),
                    "[general] ticket_workflow_steps_file not found — ignored (workflows are now defined as YAML files)"
                );
            } else {
                let raw = std::fs::read_to_string(&p)?;
                let file: TicketWorkflowStepsFile = toml::from_str(&raw)?;
                self.agent_steps = file.agent_steps;
            }
        }

        if !self.general.review_workflow_steps_file.trim().is_empty() {
            let p = resolve_config_relative_path(
                config_file_dir,
                &self.general.review_workflow_steps_file,
            );
            if !p.is_file() {
                warn!(
                    path = %p.display(),
                    "[general] review_workflow_steps_file not found — ignored (workflows are now defined as YAML files)"
                );
            } else {
                let raw = std::fs::read_to_string(&p)?;
                let file: ReviewWorkflowStepsFile = toml::from_str(&raw)?;
                self.review_agent_steps = file.review_agent_steps;
            }
        }

        if !self
            .general
            .merge_base_workflow_steps_file
            .trim()
            .is_empty()
        {
            let p = resolve_config_relative_path(
                config_file_dir,
                &self.general.merge_base_workflow_steps_file,
            );
            if !p.is_file() {
                warn!(
                    path = %p.display(),
                    "[general] merge_base_workflow_steps_file not found — ignored (workflows are now defined as YAML files)"
                );
            } else {
                let raw = std::fs::read_to_string(&p)?;
                let file: MergeBaseWorkflowStepsFile = toml::from_str(&raw)?;
                self.merge_base_agent_steps = file.merge_base_agent_steps;
            }
        }

        Ok(())
    }

    /// Outer loop count for the agent step sequence. Always `1` — use each step's `repeat` for multiple runs.
    pub fn agent_sequence_outer_loops(&self) -> u8 {
        1
    }

    /// Steps to run each outer loop (configured or [`default_agent_steps`]).
    pub fn resolved_agent_steps(&self) -> Vec<AgentStepConfig> {
        if self.agent_steps.is_empty() {
            default_agent_steps()
        } else {
            self.agent_steps.clone()
        }
    }

    /// When `review_agent_steps` is empty, the built-in review sequence runs once per **Address PR comments** run (use per-step `repeat` in `[[review_agent_steps]]` for more).
    pub fn review_sequence_outer_loops(&self) -> u8 {
        1
    }

    /// Steps for the PR-comment workflow (configured or [`default_review_agent_steps`]).
    pub fn resolved_review_agent_steps(&self) -> Vec<AgentStepConfig> {
        if self.review_agent_steps.is_empty() {
            default_review_agent_steps()
        } else {
            self.review_agent_steps.clone()
        }
    }

    /// Steps for the merge-base-branch workflow (configured or [`default_merge_base_agent_steps`]).
    pub fn resolved_merge_base_agent_steps(&self) -> Vec<AgentStepConfig> {
        if self.merge_base_agent_steps.is_empty() {
            default_merge_base_agent_steps()
        } else {
            self.merge_base_agent_steps.clone()
        }
    }

    /// Parse a `Config` from a TOML string without loading from disk.
    ///
    /// Useful for tests and scenarios where the config content is already in
    /// memory. Applies validation but **not** external workflow step files
    /// (those require a filesystem path).
    pub fn load_from_str(toml_content: &str) -> Result<Self> {
        let mut config: Config = toml::from_str(toml_content)?;
        config.web.normalize_cors_origins();
        config.validate()?;
        Ok(config)
    }

    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Err(MaestroError::ConfigNotFound(path.to_path_buf()));
        }

        let content = std::fs::read_to_string(path)?;
        let mut config: Config = toml::from_str(&content)?;
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        config.apply_workflow_step_files(base)?;
        config.web.normalize_cors_origins();
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

        // Validate run commands: names must be non-empty and unique.
        for (i, rc) in self.run_commands.iter().enumerate() {
            if rc.name.trim().is_empty() {
                return Err(MaestroError::Config(format!(
                    "[[run_commands]] entry {i} has an empty name"
                )));
            }
            if rc.command.trim().is_empty() {
                return Err(MaestroError::Config(format!(
                    "[[run_commands]] entry '{}' has an empty command",
                    rc.name
                )));
            }
        }
        {
            let mut seen_names = std::collections::HashSet::new();
            for rc in &self.run_commands {
                let key = rc.name.trim().to_lowercase();
                if !seen_names.insert(key) {
                    return Err(MaestroError::Config(format!(
                        "[[run_commands]] duplicate name: '{}'",
                        rc.name.trim()
                    )));
                }
            }
        }

        validate_step_list(&self.agent_steps, "agent_steps")?;
        validate_step_list(&self.review_agent_steps, "review_agent_steps")?;
        validate_step_list(&self.merge_base_agent_steps, "merge_base_agent_steps")?;

        if self.agent.step_timeout_secs == 0 {
            return Err(MaestroError::Config(
                "step_timeout_secs must be at least 1".to_string(),
            ));
        }

        if self.agent.provider == AiAgentProvider::Cursor && self.agent.cursor_cli.trim().is_empty()
        {
            return Err(MaestroError::Config(
                "agent.cursor_cli must be set when agent.provider is \"cursor\"".to_string(),
            ));
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

[commands]
pre_install = []

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
    fn resolved_agent_steps_default_when_empty() {
        let config = Config::default();
        assert_eq!(config.agent_steps.len(), 0);
        let steps = config.resolved_agent_steps();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].name, "Implement ticket");
        assert_eq!(steps[0].repeat, 1);
        assert_eq!(steps[1].name, "Review changes");
        assert_eq!(steps[1].repeat, 1);
    }

    #[test]
    fn agent_sequence_outer_loops_is_always_one() {
        let config = Config::default();
        assert_eq!(config.agent_sequence_outer_loops(), 1);

        let mut custom = Config::default();
        custom.agent_steps.push(AgentStepConfig {
            name: "Only".into(),
            prompt: "x".into(),
            repeat: 1,
            skills: Vec::new(),
            resume_previous: false,
            when: StepAvailability::Always,
            commands: Vec::new(),
        });
        assert_eq!(custom.agent_sequence_outer_loops(), 1);
    }

    #[test]
    fn review_sequence_outer_loops_and_defaults() {
        let config = Config::default();
        assert_eq!(config.review_sequence_outer_loops(), 1);
        let steps = config.resolved_review_agent_steps();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].name, "Address PR feedback");
        assert!(steps[0].prompt.contains("{pr_url}"));

        let mut custom = Config::default();
        custom.review_agent_steps.push(AgentStepConfig {
            name: "R".into(),
            prompt: "p".into(),
            repeat: 1,
            skills: Vec::new(),
            resume_previous: false,
            when: StepAvailability::Always,
            commands: Vec::new(),
        });
        assert_eq!(custom.review_sequence_outer_loops(), 1);
    }

    #[test]
    fn test_pre_install_string_compat() {
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
pre_install = "echo one"

[web]
port = 8080

[agent]
step_timeout_secs = 600
"#,
        )
        .unwrap();
        let config = Config::load(f.path()).unwrap();
        assert_eq!(config.commands.pre_install, vec!["echo one".to_string()]);
    }

    #[test]
    fn test_pre_install_array() {
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
pre_install = ["echo a", "echo b"]

[web]
port = 8080

[agent]
step_timeout_secs = 600
"#,
        )
        .unwrap();
        let config = Config::load(f.path()).unwrap();
        assert_eq!(
            config.commands.pre_install,
            vec!["echo a".to_string(), "echo b".to_string()]
        );
    }

    #[test]
    fn test_load_agent_steps_toml() {
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

[commands]
pre_install = []

[web]
port = 8080

[agent]
step_timeout_secs = 600

[[agent_steps]]
name = "Plan"
prompt = "Plan work for {ticket_key}"
repeat = 2

[[agent_steps]]
name = "Build"
prompt = "Implement {ticket_summary}"
"#,
        )
        .unwrap();
        let config = Config::load(f.path()).unwrap();
        assert_eq!(config.agent_steps.len(), 2);
        assert_eq!(config.agent_steps[0].name, "Plan");
        assert_eq!(config.agent_steps[0].repeat, 2);
        assert_eq!(config.agent_steps[1].name, "Build");
        assert_eq!(config.agent_steps[1].repeat, 1);
    }

    #[test]
    fn test_load_ticket_steps_from_external_file() {
        let dir = tempfile::tempdir().unwrap();
        let steps_path = dir.path().join("ticket-steps.toml");
        std::fs::write(
            &steps_path,
            br#"[[agent_steps]]
name = "From file"
prompt = "Do {ticket_key}"
repeat = 1
"#,
        )
        .unwrap();

        let main_path = dir.path().join("config.toml");
        std::fs::write(
            &main_path,
            format!(
                r#"
[general]
poll_interval_secs = 30
max_concurrent_workflows = 1
ticket_workflow_steps_file = "{}"

[jira]
project_keys = ["X"]
item_types = ["Task"]

[git]
base_branch = "main"
repo_path = "/workspace"

[commands]
pre_install = []

[web]
port = 8080

[agent]
step_timeout_secs = 600
"#,
                steps_path.file_name().unwrap().to_str().unwrap()
            ),
        )
        .unwrap();

        let config = Config::load(&main_path).unwrap();
        assert_eq!(config.agent_steps.len(), 1);
        assert_eq!(config.agent_steps[0].name, "From file");
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

    #[test]
    fn default_agent_steps_are_not_command_steps() {
        for step in default_agent_steps() {
            assert!(
                !step.is_command_step(),
                "default step {:?} should not be a command step",
                step.name
            );
        }
        for step in default_review_agent_steps() {
            assert!(!step.is_command_step());
        }
        for step in default_merge_base_agent_steps() {
            assert!(!step.is_command_step());
        }
    }

    #[test]
    fn validate_accepts_command_only_step() {
        let mut config = Config::default();
        config.agent_steps.push(AgentStepConfig {
            name: "Lint".into(),
            prompt: String::new(),
            repeat: 1,
            skills: Vec::new(),
            resume_previous: false,
            when: StepAvailability::Always,
            commands: vec!["npm run lint".into()],
        });
        assert!(config.validate().is_ok());
    }

    #[test]
    fn validate_rejects_step_with_both_prompt_and_commands() {
        let mut config = Config::default();
        config.agent_steps.push(AgentStepConfig {
            name: "Bad".into(),
            prompt: "do stuff".into(),
            repeat: 1,
            skills: Vec::new(),
            resume_previous: false,
            when: StepAvailability::Always,
            commands: vec!["npm test".into()],
        });
        let err = config.validate().unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("cannot specify both")
                && msg.contains("commands")
                && msg.contains("prompt"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn validate_rejects_step_with_both_skills_and_commands() {
        let mut config = Config::default();
        config.agent_steps.push(AgentStepConfig {
            name: "Bad".into(),
            prompt: String::new(),
            repeat: 1,
            skills: vec![SkillRef {
                name: "my-skill".into(),
                args: Vec::new(),
            }],
            resume_previous: false,
            when: StepAvailability::Always,
            commands: vec!["npm test".into()],
        });
        let err = config.validate().unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("cannot specify both")
                && msg.contains("commands")
                && msg.contains("skills"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn validate_rejects_step_with_no_prompt_no_skills_no_commands() {
        let mut config = Config::default();
        config.agent_steps.push(AgentStepConfig {
            name: "Empty".into(),
            prompt: String::new(),
            repeat: 1,
            skills: Vec::new(),
            resume_previous: false,
            when: StepAvailability::Always,
            commands: Vec::new(),
        });
        let err = config.validate().unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("must have"), "unexpected error: {msg}");
    }

    #[test]
    fn validate_command_step_in_review_steps() {
        let mut config = Config::default();
        config.review_agent_steps.push(AgentStepConfig {
            name: "Run checks".into(),
            prompt: String::new(),
            repeat: 1,
            skills: Vec::new(),
            resume_previous: false,
            when: StepAvailability::Always,
            commands: vec!["cargo test".into()],
        });
        assert!(config.validate().is_ok());
    }

    #[test]
    fn validate_command_step_in_merge_base_steps() {
        let mut config = Config::default();
        config.merge_base_agent_steps.push(AgentStepConfig {
            name: "Run checks".into(),
            prompt: String::new(),
            repeat: 1,
            skills: Vec::new(),
            resume_previous: false,
            when: StepAvailability::Always,
            commands: vec!["cargo test".into()],
        });
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_load_command_step_from_toml() {
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

[commands]
pre_install = []

[web]
port = 8080

[agent]
step_timeout_secs = 600

[[agent_steps]]
name = "Implement"
prompt = "Do {ticket_key}"

[[agent_steps]]
name = "Lint and format"
commands = [
  "npm run lint --fix",
  "npm run format"
]

[[agent_steps]]
name = "Run tests"
commands = ["npm test"]
repeat = 2
"#,
        )
        .unwrap();
        let config = Config::load(f.path()).unwrap();
        assert_eq!(config.agent_steps.len(), 3);

        // First step is a normal agent step
        assert!(!config.agent_steps[0].is_command_step());
        assert_eq!(config.agent_steps[0].prompt, "Do {ticket_key}");

        // Second step is a command step
        assert!(config.agent_steps[1].is_command_step());
        assert_eq!(
            config.agent_steps[1].commands,
            vec!["npm run lint --fix", "npm run format"]
        );
        assert!(config.agent_steps[1].prompt.is_empty());

        // Third step is a command step with repeat
        assert!(config.agent_steps[2].is_command_step());
        assert_eq!(config.agent_steps[2].commands, vec!["npm test"]);
        assert_eq!(config.agent_steps[2].repeat, 2);
    }

    #[test]
    fn test_load_command_step_from_external_file() {
        let dir = tempfile::tempdir().unwrap();
        let steps_path = dir.path().join("ticket-steps.toml");
        std::fs::write(
            &steps_path,
            br#"[[agent_steps]]
name = "Lint"
commands = ["npm run lint"]
"#,
        )
        .unwrap();

        let main_path = dir.path().join("config.toml");
        std::fs::write(
            &main_path,
            format!(
                r#"
[general]
poll_interval_secs = 30
max_concurrent_workflows = 1
ticket_workflow_steps_file = "{}"

[jira]
project_keys = ["X"]
item_types = ["Task"]

[git]
base_branch = "main"
repo_path = "/workspace"

[commands]
pre_install = []

[web]
port = 8080

[agent]
step_timeout_secs = 600
"#,
                steps_path.file_name().unwrap().to_str().unwrap()
            ),
        )
        .unwrap();

        let config = Config::load(&main_path).unwrap();
        assert_eq!(config.agent_steps.len(), 1);
        assert!(config.agent_steps[0].is_command_step());
        assert_eq!(config.agent_steps[0].commands, vec!["npm run lint"]);
    }

    #[test]
    fn validate_rejects_prompt_and_commands_in_review_steps() {
        let mut config = Config::default();
        config.review_agent_steps.push(AgentStepConfig {
            name: "Bad".into(),
            prompt: "do stuff".into(),
            repeat: 1,
            skills: Vec::new(),
            resume_previous: false,
            when: StepAvailability::Always,
            commands: vec!["npm test".into()],
        });
        let err = config.validate().unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("cannot specify both") && msg.contains("review_agent_steps"),
            "unexpected error: {msg}"
        );
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
[commands]
pre_install = []
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
[commands]
pre_install = []
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

[commands]
pre_install = []

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
}
