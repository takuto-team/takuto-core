//! Config-driven shell hooks for Docker image build and container startup.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value as JsonValue;

use crate::config::{AiAgentProvider, Config};
use crate::error::{MaestroError, Result};

fn preflight_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/home/maestro"))
}

/// Cursor CLI stores browser-login state under `CURSOR_CONFIG_DIR` (default `~/.cursor`).
/// `agent status` often returns non-zero without a TTY even when login succeeded, and the JSON schema
/// for tokens changes between releases — so we also accept “this tree clearly has Cursor CLI data”.
fn cursor_agent_auth_likely_on_disk() -> bool {
    let config_dir = std::env::var_os("CURSOR_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| preflight_home().join(".cursor"));

    let mut paths = vec![config_dir.join("cli-config.json")];
    if let Some(x) = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from) {
        paths.push(x.join("cursor/cli-config.json"));
    } else {
        paths.push(preflight_home().join(".config/cursor/cli-config.json"));
    }

    for p in &paths {
        if json_config_suggests_auth(p) {
            return true;
        }
    }

    // Any other *.json next to cli-config (Cursor versions may rename or split fields)
    if let Ok(rd) = std::fs::read_dir(&config_dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_file()
                && p.extension().and_then(|s| s.to_str()) == Some("json")
                && !paths.iter().any(|known| known == &p)
                && json_config_suggests_auth(&p)
            {
                return true;
            }
        }
    }

    // Browser login may store state in nested dirs / non-JSON files; `agent status` is unreliable headless.
    let xdg_cursor = preflight_home().join(".config/Cursor");
    let xdg_cursor_lower = preflight_home().join(".config/cursor");
    cursor_data_tree_looks_populated(&config_dir)
        || cursor_data_tree_looks_populated(&xdg_cursor)
        || cursor_data_tree_looks_populated(&xdg_cursor_lower)
}

/// True if the directory contains a small amount of non-trivial file data typical after `agent login` / CLI use.
fn cursor_data_tree_looks_populated(root: &Path) -> bool {
    if !root.is_dir() {
        return false;
    }

    fn walk(dir: &Path, depth: u8) -> bool {
        if depth > 10 {
            return false;
        }
        let Ok(rd) = std::fs::read_dir(dir) else {
            return false;
        };
        for ent in rd.flatten() {
            let p = ent.path();
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let low = name.to_lowercase();
            if low == ".ds_store" || low.contains("readme") {
                continue;
            }
            if p.is_dir() {
                if walk(&p, depth + 1) {
                    return true;
                }
            } else if let Ok(meta) = p.metadata() {
                if !meta.is_file() {
                    continue;
                }
                let len = meta.len();
                if len < 16 {
                    continue;
                }
                if low.ends_with(".log") && len < 256 {
                    continue;
                }
                // SQLite / VS Code style state DBs
                if low.ends_with(".vscdb") || low.ends_with(".db") {
                    return true;
                }
                if low.ends_with(".json") {
                    if let Ok(raw) = std::fs::read_to_string(&p) {
                        if let Ok(v) = serde_json::from_str::<JsonValue>(&raw) {
                            if json_value_has_auth_fields(&v) {
                                return true;
                            }
                            if v.as_object().is_some_and(|m| m.len() >= 2 && len >= 32) {
                                return true;
                            }
                        }
                    }
                    continue;
                }
                // Any other non-trivial file (e.g. binary token blob)
                if len >= 48 {
                    return true;
                }
            }
        }
        false
    }

    walk(root, 0)
}

fn json_config_suggests_auth(path: &Path) -> bool {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(v) = serde_json::from_str::<JsonValue>(&raw) else {
        return false;
    };
    json_value_has_auth_fields(&v)
}

fn json_value_has_auth_fields(v: &JsonValue) -> bool {
    match v {
        JsonValue::Object(map) => {
            // Cursor may store opaque session strings without "token" in the key name.
            for val in map.values() {
                if val.as_str().is_some_and(|s| s.len() >= 64) {
                    return true;
                }
            }
            for (k, val) in map {
                let kl = k.to_lowercase();
                if kl.contains("token") || kl.ends_with("apikey") || kl == "api_key" {
                    if val.as_str().is_some_and(|s| !s.trim().is_empty()) {
                        return true;
                    }
                }
            }
            map.values().any(json_value_has_auth_fields)
        }
        JsonValue::Array(items) => items.iter().any(json_value_has_auth_fields),
        JsonValue::String(s) if s.len() >= 64 => true,
        _ => false,
    }
}

#[cfg(unix)]
fn configure_auth_command_unix(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        cmd.pre_exec(|| {
            libc::setpgid(0, 0);
            Ok(())
        });
    }
}

#[cfg(not(unix))]
fn configure_auth_command_unix(_cmd: &mut Command) {}

#[cfg(unix)]
fn kill_process_group_best_effort(child: &mut std::process::Child) {
    let pid = child.id();
    if pid > 0 {
        unsafe {
            let _ = libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(not(unix))]
fn kill_process_group_best_effort(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

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
            "[maestro docker-hooks:{label}] ({n}/{total}) cwd={} script={}{} ({} bytes)",
            cwd.display(),
            preview,
            dots,
            cmd_line.len()
        );
        eprintln!("[maestro docker-hooks:{label}] ({n}/{total}) running…");
        // Prefer MAESTRO_HOME so hooks writing "$HOME/.claude" land on the named volume, not on ephemeral
        // paths like /.claude when HOME is missing under Podman/rootless.
        let home = std::env::var("MAESTRO_HOME")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_else(|_| "/home/maestro".to_string());
        let cursor_dir = std::env::var("CURSOR_CONFIG_DIR")
            .unwrap_or_else(|_| format!("{}/.cursor", home.trim_end_matches('/')));
        let mut cmd = Command::new("/bin/bash");
        cmd.arg("-c")
            .arg(cmd_line)
            .current_dir(cwd)
            .env("HOME", &home)
            .env("MAESTRO_HOME", &home)
            .env("CURSOR_CONFIG_DIR", &cursor_dir)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        let status = cmd
            .status()
            .map_err(|e| {
                MaestroError::Config(format!("failed to spawn {label} hook {n}: {e}"))
            })?;
        if !status.success() {
            return Err(MaestroError::Config(format!(
                "{label} hook command {n} failed with status {status}"
            )));
        }
        eprintln!("[maestro docker-hooks:{label}] ({n}/{total}) finished successfully.");
    }
    Ok(())
}

/// Run an auth probe with a wall-clock timeout so `docker compose up` cannot hang forever
/// (e.g. Cursor `agent status` waiting on network without a client-side deadline).
fn auth_cmd_ok(program: &str, args: &[&str]) -> bool {
    let timeout = if args == ["status"] {
        Duration::from_secs(45)
    } else {
        Duration::from_secs(30)
    };

    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    configure_auth_command_unix(&mut cmd);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => return false,
    };

    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => {}
            Err(_) => {
                kill_process_group_best_effort(&mut child);
                return false;
            }
        }
        if start.elapsed() >= timeout {
            kill_process_group_best_effort(&mut child);
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Verify required CLIs for the configured AI provider. Used before `docker compose up`.
pub fn preflight(config: &Config) -> Result<()> {
    eprintln!("[maestro preflight] Checking GitHub CLI (gh)…");
    if !auth_cmd_ok("gh", &["auth", "status"]) {
        return Err(MaestroError::Config(
            "GitHub CLI (gh) is not authenticated. Run: docker compose run --rm -it maestro setup"
                .to_string(),
        ));
    }

    eprintln!("[maestro preflight] Checking Atlassian CLI (acli)…");
    if !auth_cmd_ok("acli", &["jira", "auth", "status"]) {
        return Err(MaestroError::Config(
            "Atlassian CLI (acli) is not authenticated. Run: docker compose run --rm -it maestro setup"
                .to_string(),
        ));
    }

    match config.agent.provider {
        AiAgentProvider::Claude => {
            eprintln!("[maestro preflight] Checking Claude Code…");
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
            if has_api_key {
                eprintln!("[maestro preflight] Cursor: CURSOR_API_KEY is set; skipping agent status probe.");
            } else if cursor_agent_auth_likely_on_disk() {
                eprintln!(
                    "[maestro preflight] Cursor: found on-disk CLI data (tokens/config tree); skipping agent status (unreliable without a TTY)."
                );
            } else {
                eprintln!(
                    "[maestro preflight] Cursor: checking {cli} status (45s timeout)…"
                );
                if !auth_cmd_ok(cli, &["status"]) {
                    return Err(MaestroError::Config(format!(
                        "Cursor Agent ({cli}) is not logged in and CURSOR_API_KEY is not set. Run: docker compose run --rm -it maestro setup (Cursor step) or set CURSOR_API_KEY in maestro.env. If you already logged in, exec into the container and check: ls -la \"$CURSOR_CONFIG_DIR\" and ~/.config/Cursor — the cursor-auth volume must be the same compose project as setup, and CURSOR_CONFIG_DIR should be /home/maestro/.cursor (see docker-compose.yml)."
                    )));
                }
            }
        }
    }

    eprintln!("[maestro preflight] OK.");
    Ok(())
}

#[cfg(test)]
mod cursor_preflight_tests {
    use std::io::Write;

    use super::*;
    use tempfile::tempdir;

    #[test]
    fn detects_opaque_session_string_in_cli_config() {
        let d = tempdir().unwrap();
        let p = d.path().join("cli-config.json");
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(br#"{"session":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#)
            .unwrap();
        assert!(json_config_suggests_auth(&p));
    }

    #[test]
    fn tree_populated_finds_nested_vscdb() {
        let d = tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("User/globalStorage")).unwrap();
        std::fs::write(
            d.path().join("User/globalStorage/state.vscdb"),
            [0u8; 64],
        )
        .unwrap();
        assert!(cursor_data_tree_looks_populated(d.path()));
    }
}
