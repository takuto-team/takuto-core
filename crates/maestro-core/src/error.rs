// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum MaestroError {
    #[error("Jira error: {0}")]
    Jira(String),

    #[error("Git error: {0}")]
    Git(String),

    #[error("GitHub App error: {0}")]
    GitHubApp(String),

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

    #[error("Database error: {0}")]
    Database(String),

    #[error("Authentication error: {0}")]
    Auth(String),

    #[error("Config error: {0}")]
    Config(String),

    #[error("Config file not found: {0}")]
    ConfigNotFound(PathBuf),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    TomlParse(#[from] toml::de::Error),
}

impl From<rusqlite::Error> for MaestroError {
    fn from(e: rusqlite::Error) -> Self {
        MaestroError::Database(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, MaestroError>;
