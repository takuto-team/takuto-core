//! Config-driven shell hooks for Docker image build and container startup.

use std::path::Path;
use std::process::{Command, Stdio};

use crate::config::{AiAgentProvider, Config};
use crate::error::{MaestroError, Result};

/// Run each non-empty command with `sh -c` in `cwd`, inheriting stdio (for logs during build/up).
pub fn run_hook_commands(commands: &[String], cwd: &Path, label: &str) -> Result<()> {
    if commands.is_empty() {
        return Ok(());
    }

    let _ = std::fs::create_dir_all(cwd);

    let total = commands.iter().filter(|c| !c.trim().is_empty()).count();
    let mut n = 0usize;
    for cmd_line in commands {
        if cmd_line.trim().is_empty() {
            continue;
        }
        n += 1;
        eprintln!("[maestro docker-hooks:{label}] ({n}/{total}) {cmd_line}");
        let status = Command::new("sh")
            .arg("-c")
            .arg(cmd_line)
            .current_dir(cwd)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .map_err(|e| {
                MaestroError::Config(format!("failed to spawn {label} hook {n}: {e}"))
            })?;
        if !status.success() {
            return Err(MaestroError::Config(format!(
                "{label} hook command {n} failed with status {status}"
            )));
        }
    }
    Ok(())
}

fn auth_cmd_ok(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Verify required CLIs for the configured AI provider. Used before `docker compose up`.
pub fn preflight(config: &Config) -> Result<()> {
    if !auth_cmd_ok("gh", &["auth", "status"]) {
        return Err(MaestroError::Config(
            "GitHub CLI (gh) is not authenticated. Run: docker compose run --rm -it maestro setup"
                .to_string(),
        ));
    }

    if !auth_cmd_ok("acli", &["jira", "auth", "status"]) {
        return Err(MaestroError::Config(
            "Atlassian CLI (acli) is not authenticated. Run: docker compose run --rm -it maestro setup"
                .to_string(),
        ));
    }

    match config.agent.provider {
        AiAgentProvider::Claude => {
            if !auth_cmd_ok("claude", &["auth", "status"]) {
                return Err(MaestroError::Config(
                    "Claude Code is not authenticated but [agent] provider is \"claude\". Run: docker compose run --rm -it maestro setup (Claude step) or switch provider."
                        .to_string(),
                ));
            }
        }
        AiAgentProvider::Cursor => {
            let cli = config.agent.cursor_cli.trim();
            if cli.is_empty() {
                return Err(MaestroError::Config(
                    "[agent] cursor_cli must be set when provider is \"cursor\"".to_string(),
                ));
            }
            let has_api_key = std::env::var("CURSOR_API_KEY")
                .map(|v| !v.trim().is_empty())
                .unwrap_or(false);
            if !has_api_key && !auth_cmd_ok(cli, &["status"]) {
                return Err(MaestroError::Config(format!(
                    "Cursor Agent ({cli}) is not logged in and CURSOR_API_KEY is not set. Run: docker compose run --rm -it maestro setup (Cursor step) or set CURSOR_API_KEY in maestro.env."
                )));
            }
        }
    }

    Ok(())
}
