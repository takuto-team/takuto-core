use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum MaestroError {
    #[error("Jira error: {0}")]
    Jira(String),

    #[error("Git error: {0}")]
    Git(String),

    #[error("Claude session error: {0}")]
    Claude(String),

    #[error("AI agent error: {0}")]
    AiAgent(String),

    #[error("Command failed: {cmd} (exit code {code})\n{stderr}")]
    Command {
        cmd: String,
        code: i32,
        stderr: String,
    },

    #[error("Timeout after {0}s")]
    Timeout(u64),

    #[error("Workflow cancelled")]
    Cancelled,

    #[error("Config error: {0}")]
    Config(String),

    #[error("Config file not found: {0}")]
    ConfigNotFound(PathBuf),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    TomlParse(#[from] toml::de::Error),
}

pub type Result<T> = std::result::Result<T, MaestroError>;
