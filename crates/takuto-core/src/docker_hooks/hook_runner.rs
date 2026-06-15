// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.
//! Config-driven `bash -c` hook executor for Docker image build and container startup.

use std::path::Path;
use std::process::{Command, Stdio};

use crate::config::ConfigError;
use crate::error::Result;

/// Run each non-empty command with `bash -c` in `cwd`, inheriting stdio (for logs during build/up).
/// Debian `sh` is often **dash**, which does not support `set -o pipefail` and other bash-isms used in hooks.
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
        let preview: String = cmd_line.chars().take(100).collect();
        let dots = if cmd_line.len() > 100 { "…" } else { "" };
        eprintln!(
            "[takuto docker-hooks:{label}] ({n}/{total}) cwd={} script={}{} ({} bytes)",
            cwd.display(),
            preview,
            dots,
            cmd_line.len()
        );
        eprintln!("[takuto docker-hooks:{label}] ({n}/{total}) running…");
        // Prefer TAKUTO_HOME so hooks writing "$HOME/.claude" land on the named volume, not on ephemeral
        // paths like /.claude when HOME is missing under Podman/rootless.
        let home = std::env::var("TAKUTO_HOME")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_else(|_| "/home/takuto".to_string());
        let cursor_dir = std::env::var("CURSOR_CONFIG_DIR")
            .unwrap_or_else(|_| format!("{}/.cursor", home.trim_end_matches('/')));
        let mut cmd = Command::new("/bin/bash");
        cmd.arg("-c")
            .arg(cmd_line)
            .current_dir(cwd)
            .env("HOME", &home)
            .env("TAKUTO_HOME", &home)
            .env("CURSOR_CONFIG_DIR", &cursor_dir)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        let status = cmd.status().map_err(|e| ConfigError::Operational {
            op: "hook spawn",
            detail: format!("{label} hook {n}: {e}"),
        })?;
        if !status.success() {
            return Err(ConfigError::Operational {
                op: "hook exit",
                detail: format!("{label} hook command {n} failed with status {status}"),
            }
            .into());
        }
        eprintln!("[takuto docker-hooks:{label}] ({n}/{total}) finished successfully.");
    }
    Ok(())
}
