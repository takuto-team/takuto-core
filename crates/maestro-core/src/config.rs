use std::fmt;
use std::path::Path;

use serde::de::{self, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use crate::error::{MaestroError, Result};

/// Which CLI implements ticket implementation / review / fix steps.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum AiAgentProvider {
    #[default]
    Claude,
    Cursor,
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
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub general: GeneralConfig,
    #[serde(default)]
    pub jira: JiraConfig,
    #[serde(default)]
    pub git: GitConfig,
    #[serde(default)]
    pub commands: CommandsConfig,
    #[serde(default)]
    pub web: WebConfig,
    #[serde(default)]
    pub claude: ClaudeConfig,
    #[serde(default)]
    pub agent: AgentConfig,
    #[serde(default)]
    pub docker: DockerConfig,
    #[serde(default)]
    pub network: NetworkConfig,
}

/// Docker-specific hooks (see README). `build_commands` run at image build time; `compose_up_commands` on each container start.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DockerConfig {
    /// Shell commands (`bash -c`) executed once while building the image, after tools are installed.
    #[serde(default)]
    pub build_commands: Vec<String>,
    /// Shell commands executed on every `docker compose up` as the maestro user, after auth preflight, before the server.
    #[serde(default)]
    pub compose_up_commands: Vec<String>,
}

impl Default for DockerConfig {
    fn default() -> Self {
        Self {
            build_commands: Vec::new(),
            compose_up_commands: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent_workflows: u32,
    #[serde(default = "default_max_fix_attempts")]
    pub max_fix_attempts: u32,
    #[serde(default = "default_log_level")]
    pub log_level: String,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitConfig {
    #[serde(default = "default_base_branch")]
    pub base_branch: String,
    #[serde(default)]
    pub repo_url: String,
    #[serde(default = "default_repo_path")]
    pub repo_path: String,
}

fn deserialize_pre_install_vec<'de, D>(deserializer: D) -> std::result::Result<Vec<String>, D::Error>
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandsConfig {
    #[serde(default, deserialize_with = "deserialize_pre_install_vec")]
    pub pre_install: Vec<String>,
    #[serde(default)]
    pub install: String,
    #[serde(default)]
    pub lint: String,
    #[serde(default)]
    pub unit_test: String,
    #[serde(default)]
    pub e2e_test: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeConfig {
    #[serde(default = "default_skills_path")]
    pub skills_path: String,
    #[serde(default = "default_address_ticket_passes")]
    pub address_ticket_passes: u8,
    #[serde(default = "default_step_timeout")]
    pub step_timeout_secs: u64,
    #[serde(default)]
    pub figma_api_token: String,
    #[serde(default)]
    pub model: String,
}

// Default value functions

fn default_poll_interval() -> u64 {
    60
}
fn default_max_concurrent() -> u32 {
    1
}
fn default_max_fix_attempts() -> u32 {
    3
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
fn default_host() -> String {
    "0.0.0.0".to_string()
}
fn default_port() -> u16 {
    8080
}
fn default_skills_path() -> String {
    "/root/.claude/skills".to_string()
}
fn default_address_ticket_passes() -> u8 {
    3
}
fn default_step_timeout() -> u64 {
    1800
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            extra_egress_hosts: Vec::new(),
            allow_all_https: false,
        }
    }
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            dry_mode: false,
            poll_interval_secs: default_poll_interval(),
            max_concurrent_workflows: default_max_concurrent(),
            max_fix_attempts: default_max_fix_attempts(),
            log_level: default_log_level(),
        }
    }
}

impl Default for JiraConfig {
    fn default() -> Self {
        Self {
            project_keys: Vec::new(),
            item_types: default_item_types(),
            jql_filter: String::new(),
            site: String::new(),
            email: String::new(),
        }
    }
}

impl Default for GitConfig {
    fn default() -> Self {
        Self {
            base_branch: default_base_branch(),
            repo_url: String::new(),
            repo_path: default_repo_path(),
        }
    }
}

impl Default for CommandsConfig {
    fn default() -> Self {
        Self {
            pre_install: Vec::new(),
            install: String::new(),
            lint: String::new(),
            unit_test: String::new(),
            e2e_test: String::new(),
        }
    }
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
        }
    }
}

impl Default for ClaudeConfig {
    fn default() -> Self {
        Self {
            skills_path: default_skills_path(),
            address_ticket_passes: default_address_ticket_passes(),
            step_timeout_secs: default_step_timeout(),
            figma_api_token: String::new(),
            model: String::new(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            general: GeneralConfig::default(),
            jira: JiraConfig::default(),
            git: GitConfig::default(),
            commands: CommandsConfig::default(),
            web: WebConfig::default(),
            claude: ClaudeConfig::default(),
            agent: AgentConfig::default(),
            docker: DockerConfig::default(),
            network: NetworkConfig::default(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Err(MaestroError::ConfigNotFound(path.to_path_buf()));
        }

        let content = std::fs::read_to_string(path)?;
        let config: Config = toml::from_str(&content)?;
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

        if self.general.max_fix_attempts == 0 {
            return Err(MaestroError::Config(
                "max_fix_attempts must be at least 1".to_string(),
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

        if self.claude.address_ticket_passes == 0 {
            return Err(MaestroError::Config(
                "address_ticket_passes must be at least 1".to_string(),
            ));
        }

        if self.claude.step_timeout_secs == 0 {
            return Err(MaestroError::Config(
                "step_timeout_secs must be at least 1".to_string(),
            ));
        }

        if self.agent.provider == AiAgentProvider::Cursor && self.agent.cursor_cli.trim().is_empty() {
            return Err(MaestroError::Config(
                "agent.cursor_cli must be set when agent.provider is \"cursor\"".to_string(),
            ));
        }

        Ok(())
    }

    pub fn to_toml_string(&self) -> Result<String> {
        toml::to_string_pretty(self)
            .map_err(|e| MaestroError::Config(format!("Failed to serialize config: {e}")))
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
poll_interval_secs = 30
max_concurrent_workflows = 2
max_fix_attempts = 3

[jira]
project_keys = ["PROJ", "CORE"]
item_types = ["Task", "Bug"]

[git]
base_branch = "main"
repo_path = "/workspace"

[commands]
pre_install = []
lint = "npm run lint"
unit_test = "npm test"
e2e_test = "npm run test:e2e"

[web]
port = 8080

[claude]
address_ticket_passes = 3
step_timeout_secs = 600
"#
    }

    #[test]
    fn test_load_valid_config() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(valid_config_toml().as_bytes()).unwrap();
        let config = Config::load(f.path()).unwrap();
        assert!(config.general.dry_mode);
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
        assert_eq!(config.general.poll_interval_secs, 60);
        assert_eq!(config.web.port, 8080);
        assert_eq!(config.agent.cursor_model, "Auto");
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

[claude]
address_ticket_passes = 3
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

[claude]
address_ticket_passes = 3
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
}
