// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::path::Path;
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::error::{MaestroError, Result};

/// True when `cwd` appears to declare mise-managed tools (project-local versions).
pub fn worktree_has_mise_config(cwd: &Path) -> bool {
    cwd.join(".mise.toml").is_file()
        || cwd.join("mise.toml").is_file()
        || cwd.join(".tool-versions").is_file()
        || cwd
            .join(".config")
            .join("mise")
            .join("config.toml")
            .is_file()
}

#[derive(Debug, Clone)]
pub struct OutputLine {
    pub content: String,
    pub stream: String, // "stdout" or "stderr"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl CommandOutput {
    pub fn success(&self) -> bool {
        self.exit_code == 0
    }
}

pub struct ProcessHandle {
    child: Child,
    pub stdout_lines: Vec<String>,
    pub stderr_lines: Vec<String>,
    cancel_token: CancellationToken,
}

/// Kill the entire process group for a child process (Unix only).
/// Falls back to killing just the child on non-Unix or if pgid kill fails.
async fn kill_process_group(child: &mut Child) {
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            // Send SIGKILL to the entire process group (negative PID)
            let pgid = pid as i32;
            unsafe {
                libc::kill(-pgid, libc::SIGKILL);
            }
        }
    }
    // Always also kill the direct child as a fallback
    let _ = child.kill().await;
}

/// Configure a command to create a new process group (Unix only).
/// This ensures all child processes can be killed together.
#[cfg(unix)]
fn set_process_group(cmd: &mut Command) {
    unsafe {
        cmd.pre_exec(|| {
            // Create a new process group with this process as the leader
            libc::setpgid(0, 0);
            Ok(())
        });
    }
}

#[cfg(not(unix))]
fn set_process_group(_cmd: &mut Command) {
    // No-op on non-Unix platforms
}

impl ProcessHandle {
    pub async fn spawn(
        program: &str,
        args: &[&str],
        cwd: &Path,
        cancel_token: CancellationToken,
    ) -> Result<Self> {
        let mut cmd = Command::new(program);
        cmd.args(args)
            .current_dir(cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null())
            .kill_on_drop(true);
        set_process_group(&mut cmd);
        let child = cmd.spawn()?;

        Ok(Self {
            child,
            stdout_lines: Vec::new(),
            stderr_lines: Vec::new(),
            cancel_token,
        })
    }

    pub async fn spawn_shell(
        command: &str,
        cwd: &Path,
        cancel_token: CancellationToken,
    ) -> Result<Self> {
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(command)
            .current_dir(cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null())
            .kill_on_drop(true);
        set_process_group(&mut cmd);
        let child = cmd.spawn()?;

        Ok(Self {
            child,
            stdout_lines: Vec::new(),
            stderr_lines: Vec::new(),
            cancel_token,
        })
    }

    /// Run `command` via `mise exec` so `.mise.toml` / `.tool-versions` apply.
    pub async fn spawn_mise_exec_shell(
        command: &str,
        cwd: &Path,
        cancel_token: CancellationToken,
    ) -> Result<Self> {
        let mut cmd = Command::new("mise");
        cmd.args(["exec", "--", "sh", "-c", command])
            .current_dir(cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null())
            .kill_on_drop(true);
        set_process_group(&mut cmd);
        let child = cmd.spawn()?;

        Ok(Self {
            child,
            stdout_lines: Vec::new(),
            stderr_lines: Vec::new(),
            cancel_token,
        })
    }

    pub async fn wait_with_output(mut self) -> Result<CommandOutput> {
        let stdout = self.child.stdout.take().expect("stdout was already taken");
        let stderr = self.child.stderr.take().expect("stderr was already taken");

        let mut stdout_reader = BufReader::new(stdout).lines();
        let mut stderr_reader = BufReader::new(stderr).lines();

        let cancel = self.cancel_token.clone();

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    warn!("Process cancelled, killing process group");
                    kill_process_group(&mut self.child).await;
                    return Err(MaestroError::Cancelled);
                }
                line = stdout_reader.next_line() => {
                    match line {
                        Ok(Some(line)) => {
                            debug!(stdout = %line);
                            self.stdout_lines.push(line);
                        }
                        Ok(None) => break,
                        Err(e) => {
                            warn!(error = %e, "Error reading stdout");
                            break;
                        }
                    }
                }
                line = stderr_reader.next_line() => {
                    match line {
                        Ok(Some(line)) => {
                            debug!(stderr = %line);
                            self.stderr_lines.push(line);
                        }
                        Ok(None) => {}
                        Err(e) => {
                            warn!(error = %e, "Error reading stderr");
                        }
                    }
                }
            }
        }

        // Drain remaining stderr
        while let Ok(Some(line)) = stderr_reader.next_line().await {
            self.stderr_lines.push(line);
        }

        let status = self.child.wait().await?;

        Ok(CommandOutput {
            exit_code: status.code().unwrap_or(-1),
            stdout: self.stdout_lines.join("\n"),
            stderr: self.stderr_lines.join("\n"),
        })
    }

    pub async fn wait_with_streaming(
        mut self,
        line_tx: tokio::sync::mpsc::UnboundedSender<OutputLine>,
    ) -> Result<CommandOutput> {
        let stdout = self.child.stdout.take().expect("stdout was already taken");
        let stderr = self.child.stderr.take().expect("stderr was already taken");

        let mut stdout_reader = BufReader::new(stdout).lines();
        let mut stderr_reader = BufReader::new(stderr).lines();

        let cancel = self.cancel_token.clone();

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    warn!("Process cancelled, killing process group");
                    kill_process_group(&mut self.child).await;
                    return Err(MaestroError::Cancelled);
                }
                line = stdout_reader.next_line() => {
                    match line {
                        Ok(Some(line)) => {
                            debug!(stdout = %line);
                            let _ = line_tx.send(OutputLine {
                                content: line.clone(),
                                stream: "stdout".to_string(),
                            });
                            self.stdout_lines.push(line);
                        }
                        Ok(None) => break,
                        Err(e) => {
                            warn!(error = %e, "Error reading stdout");
                            break;
                        }
                    }
                }
                line = stderr_reader.next_line() => {
                    match line {
                        Ok(Some(line)) => {
                            debug!(stderr = %line);
                            let _ = line_tx.send(OutputLine {
                                content: line.clone(),
                                stream: "stderr".to_string(),
                            });
                            self.stderr_lines.push(line);
                        }
                        Ok(None) => {}
                        Err(e) => {
                            warn!(error = %e, "Error reading stderr");
                        }
                    }
                }
            }
        }

        // Drain remaining stderr
        while let Ok(Some(line)) = stderr_reader.next_line().await {
            let _ = line_tx.send(OutputLine {
                content: line.clone(),
                stream: "stderr".to_string(),
            });
            self.stderr_lines.push(line);
        }

        let status = self.child.wait().await?;

        Ok(CommandOutput {
            exit_code: status.code().unwrap_or(-1),
            stdout: self.stdout_lines.join("\n"),
            stderr: self.stderr_lines.join("\n"),
        })
    }

    pub async fn wait_with_timeout(self, timeout_secs: u64) -> Result<CommandOutput> {
        // Capture the child PID before self is consumed, so we can kill the
        // process group from the timeout branch.
        #[cfg(unix)]
        let child_pid = self.child.id();

        tokio::select! {
            result = self.wait_with_output() => result,
            _ = tokio::time::sleep(std::time::Duration::from_secs(timeout_secs)) => {
                warn!(timeout_secs = timeout_secs, "Process timed out, killing process group");
                #[cfg(unix)]
                if let Some(pid) = child_pid {
                    unsafe { libc::kill(-(pid as i32), libc::SIGKILL); }
                }
                Err(MaestroError::Timeout(timeout_secs))
            }
        }
    }

    pub async fn wait_with_streaming_timeout(
        self,
        timeout_secs: u64,
        line_tx: tokio::sync::mpsc::UnboundedSender<OutputLine>,
    ) -> Result<CommandOutput> {
        #[cfg(unix)]
        let child_pid = self.child.id();

        tokio::select! {
            result = self.wait_with_streaming(line_tx) => result,
            _ = tokio::time::sleep(std::time::Duration::from_secs(timeout_secs)) => {
                warn!(timeout_secs = timeout_secs, "Process timed out, killing process group");
                #[cfg(unix)]
                if let Some(pid) = child_pid {
                    unsafe { libc::kill(-(pid as i32), libc::SIGKILL); }
                }
                Err(MaestroError::Timeout(timeout_secs))
            }
        }
    }
}

async fn spawn_shell_in_worktree(
    command: &str,
    cwd: &Path,
    cancel_token: CancellationToken,
) -> Result<ProcessHandle> {
    if worktree_has_mise_config(cwd) {
        ProcessHandle::spawn_mise_exec_shell(command, cwd, cancel_token).await
    } else {
        ProcessHandle::spawn_shell(command, cwd, cancel_token).await
    }
}

pub async fn run_shell_command(
    command: &str,
    cwd: &Path,
    cancel_token: CancellationToken,
) -> Result<CommandOutput> {
    let handle = spawn_shell_in_worktree(command, cwd, cancel_token).await?;
    handle.wait_with_output().await
}

/// Run a command with explicit args, bypassing shell interpretation.
/// Use this when arguments contain quotes or special characters.
pub async fn run_command(
    program: &str,
    args: &[&str],
    cwd: &Path,
    cancel_token: CancellationToken,
) -> Result<CommandOutput> {
    let handle = ProcessHandle::spawn(program, args, cwd, cancel_token).await?;
    handle.wait_with_output().await
}

pub async fn run_shell_command_streaming(
    command: &str,
    cwd: &Path,
    cancel_token: CancellationToken,
    line_tx: tokio::sync::mpsc::UnboundedSender<OutputLine>,
) -> Result<CommandOutput> {
    let handle = spawn_shell_in_worktree(command, cwd, cancel_token).await?;
    handle.wait_with_streaming(line_tx).await
}

pub async fn run_command_streaming(
    program: &str,
    args: &[&str],
    cwd: &Path,
    cancel_token: CancellationToken,
    line_tx: tokio::sync::mpsc::UnboundedSender<OutputLine>,
) -> Result<CommandOutput> {
    let handle = ProcessHandle::spawn(program, args, cwd, cancel_token).await?;
    handle.wait_with_streaming(line_tx).await
}

/// Run a shell command with streaming output and a timeout.
pub async fn run_shell_command_streaming_with_timeout(
    command: &str,
    cwd: &Path,
    cancel_token: CancellationToken,
    line_tx: tokio::sync::mpsc::UnboundedSender<OutputLine>,
    timeout_secs: u64,
) -> Result<CommandOutput> {
    let handle = spawn_shell_in_worktree(command, cwd, cancel_token).await?;
    handle
        .wait_with_streaming_timeout(timeout_secs, line_tx)
        .await
}

/// Run a command with explicit args, streaming output, and a timeout.
pub async fn run_command_streaming_with_timeout(
    program: &str,
    args: &[&str],
    cwd: &Path,
    cancel_token: CancellationToken,
    line_tx: tokio::sync::mpsc::UnboundedSender<OutputLine>,
    timeout_secs: u64,
) -> Result<CommandOutput> {
    let handle = ProcessHandle::spawn(program, args, cwd, cancel_token).await?;
    handle
        .wait_with_streaming_timeout(timeout_secs, line_tx)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn worktree_has_mise_config_mise_toml() {
        let dir = tempdir().unwrap();
        let p = dir.path();
        assert!(!worktree_has_mise_config(p));
        fs::write(p.join(".mise.toml"), "[tools]\n").unwrap();
        assert!(worktree_has_mise_config(p));
    }

    #[test]
    fn worktree_has_mise_config_tool_versions() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join(".tool-versions"), "node 22\n").unwrap();
        assert!(worktree_has_mise_config(dir.path()));
    }

    #[test]
    fn worktree_has_mise_config_nested_config() {
        let dir = tempdir().unwrap();
        let cfg = dir.path().join(".config").join("mise");
        fs::create_dir_all(&cfg).unwrap();
        fs::write(cfg.join("config.toml"), "[tools]\n").unwrap();
        assert!(worktree_has_mise_config(dir.path()));
    }
}
