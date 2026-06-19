// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! User-defined run-commands (`[[run_commands]]` config). Each command runs as
//! a `docker exec` process **inside the per-item workspace container**
//! ([`super::workspace`]) rather than owning a container — so the IDE,
//! terminal, dev servers and the agent's worktree all share one filesystem
//! (reliable hot-reload) and one runtime dir (cross-command URL handoff).
//!
//! Lifecycle is tracked with files under `$TAKUTO_RUNTIME_DIR`: a `.pid` (the
//! command's session leader, started under `setsid` so the whole process group
//! can be signalled), a `.log`, and a `.exit` (written when it finishes). A
//! per-command port scanner forwards detected dev-server ports via socat.

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;

use tokio::io::AsyncWriteExt;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::workflow::engine::WorkflowEvent;

use super::editor::{EDITOR_PORT_MAX, EDITOR_PORT_MIN, allocate_editor_ports};
use super::editor_host_port;
use super::port_scanner::{kill_socat, scan_listening_ports, start_socat};
use super::workspace::{RUNTIME_DIR, ensure_workspace_container, workspace_container_name};
use super::{sanitize_ticket_key, shell_escape};

/// Legacy per-command container name. Run-commands no longer own a container
/// (they exec into the workspace container); retained for the
/// `stop_all_run_commands` sweep that reaps any leftover from older versions.
pub fn run_command_container_name(ticket_key: &str, cmd_index: usize) -> String {
    format!(
        "takuto-run-{}-{}",
        sanitize_ticket_key(ticket_key),
        cmd_index
    )
}

/// Runtime-dir file paths (inside the workspace container) for a command.
fn pid_path(cmd_index: usize) -> String {
    format!("{RUNTIME_DIR}/run-{cmd_index}.pid")
}
fn exit_path(cmd_index: usize) -> String {
    format!("{RUNTIME_DIR}/run-{cmd_index}.exit")
}
fn log_path(cmd_index: usize) -> String {
    format!("{RUNTIME_DIR}/run-{cmd_index}.log")
}
fn script_path(cmd_index: usize) -> String {
    format!("{RUNTIME_DIR}/run-{cmd_index}.sh")
}
/// Per-command service registry file (goal: cross-command URL handoff). Holds
/// this command's forwarded ports so sibling run-commands / tests in the same
/// workspace container can discover them under `$TAKUTO_RUNTIME_DIR`.
fn service_path(cmd_index: usize) -> String {
    format!("{RUNTIME_DIR}/service-{cmd_index}.json")
}

/// Serialize a command's active forwards (`container_port → host_port`) to the
/// JSON published in its `service-<idx>.json`.
fn service_json(cmd_index: usize, forwards: &HashMap<u16, u16>) -> String {
    let mut list: Vec<(u16, u16)> = forwards
        .iter()
        .map(|(&detected, &spare)| (detected, editor_host_port(spare)))
        .collect();
    list.sort_unstable();
    let services: Vec<serde_json::Value> = list
        .into_iter()
        .map(|(container_port, host_port)| {
            serde_json::json!({
                "container_port": container_port,
                "host_port": host_port,
                "url": format!("http://localhost:{container_port}"),
            })
        })
        .collect();
    serde_json::json!({ "command_index": cmd_index, "services": services }).to_string()
}

/// Information about a running run-command.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RunCommandInfo {
    /// Index of the command in the `[[run_commands]]` config array.
    pub index: usize,
    /// The host port on which the detected application port is forwarded, if any.
    pub forwarded_port: Option<(u16, u16)>,
}

/// Build the launcher script that runs the user `command` in the workspace
/// container. Written to `script_path` via stdin (so the user command needs no
/// escaping) and `setsid`-launched by [`start_run_command`].
///
/// The script records its own pid (`$$` — the session leader, since it runs
/// under `setsid`) so stop can signal the whole process group, and writes its
/// exit code when the command finishes (a long-running dev server simply never
/// reaches that line, which reads as "still running").
fn build_command_script(
    worktree_path: &Path,
    command: &str,
    extra_env: &[(&str, &str)],
    cmd_index: usize,
) -> String {
    let mut s = String::from("#!/bin/bash\n");
    s.push_str(&format!("echo $$ > {}\n", pid_path(cmd_index)));
    s.push_str("set -a\n");
    s.push_str(&format!(
        "cd {} 2>/dev/null || true\n",
        worktree_path.to_string_lossy()
    ));
    s.push_str("[ -f /etc/takuto/env ] && . /etc/takuto/env\n");
    for (k, v) in extra_env {
        s.push_str(&format!("export {k}={}\n", shell_escape(v)));
    }
    s.push_str("set +a\n");
    s.push_str(command);
    s.push('\n');
    s.push_str(&format!("echo $? > {}\n", exit_path(cmd_index)));
    s
}

/// Write `content` to `path` inside `container` via `docker exec -i` (stdin),
/// avoiding any shell escaping of the content.
async fn write_file_in_container(container: &str, path: &str, content: &str) -> Result<(), String> {
    let mut child = tokio::process::Command::new("docker")
        .args([
            "exec",
            "-i",
            container,
            "sh",
            "-c",
            &format!("mkdir -p {RUNTIME_DIR} && cat > {path}"),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn docker exec for script write: {e}"))?;
    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "docker exec stdin unavailable".to_string())?;
        stdin
            .write_all(content.as_bytes())
            .await
            .map_err(|e| format!("writing run-command script to stdin failed: {e}"))?;
        // Drop stdin (EOF) so `cat` finishes and the exec exits.
        stdin
            .shutdown()
            .await
            .map_err(|e| format!("closing run-command script stdin failed: {e}"))?;
    }
    let out = child
        .wait_with_output()
        .await
        .map_err(|e| format!("docker exec script write failed: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "writing run-command script failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(())
}

/// Start a run-command as a `docker exec` process inside the item's workspace
/// container. Ensures the container exists first, then launches the user
/// command under `setsid`, recording its pid/exit in `$TAKUTO_RUNTIME_DIR`.
/// Returns the spare host ports allocated for dynamic port forwarding.
#[allow(clippy::too_many_arguments)]
pub async fn start_run_command(
    ticket_key: &str,
    worktree_path: &Path,
    image: &str,
    command: &str,
    cmd_index: usize,
    dynamic_ports: usize,
    isolate_workspace: bool,
    extra_env: &[(&str, &str)],
    secrets_bundle: Option<&crate::auth::WorkerSecretsBundle>,
    // Workspace init commands — run when the workspace container is brought up.
    init_commands: &[String],
) -> std::result::Result<Vec<u16>, String> {
    // Spare ports for socat forwarding. Allocated here and published when the
    // workspace container is created (local mode); in DinD (`--network=host`)
    // they are reachable in the shared netns without publishing.
    let count = dynamic_ports.max(1);
    let spare_ports = allocate_editor_ports(count).await.ok_or_else(|| {
        format!(
            "No free ports available for run command (range {EDITOR_PORT_MIN}–{EDITOR_PORT_MAX})"
        )
    })?;

    ensure_workspace_container(
        ticket_key,
        worktree_path,
        image,
        isolate_workspace,
        secrets_bundle,
        &spare_ports,
        init_commands,
    )
    .await?;

    let name = workspace_container_name(ticket_key);

    if is_run_command_running(ticket_key, cmd_index).await {
        return Err(format!(
            "Run command {cmd_index} is already running for {ticket_key}"
        ));
    }

    // Write the user command to a script file (no escaping needed via stdin).
    let script = build_command_script(worktree_path, command, extra_env, cmd_index);
    write_file_in_container(&name, &script_path(cmd_index), &script).await?;

    // Launch detached under setsid: a new session (so it survives the launching
    // shell exiting and stop can signal the whole group). The script records
    // its own pid + exit code (see build_command_script).
    let launcher = format!(
        "rm -f {exit} {pid}; chmod +x {script}; setsid bash {script} >{log} 2>&1 &",
        exit = exit_path(cmd_index),
        pid = pid_path(cmd_index),
        script = script_path(cmd_index),
        log = log_path(cmd_index),
    );

    info!(ticket = %ticket_key, cmd_index, container = %name, "Starting run command (exec)");
    let output = tokio::process::Command::new("docker")
        .args(["exec", "-d", &name, "bash", "-lc", &launcher])
        .output()
        .await
        .map_err(|e| format!("Failed to launch run command: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "run command launch failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    Ok(spare_ports)
}

/// Check whether a run-command's process is still alive inside the workspace
/// container: its pid file exists, the pid responds to `kill -0`, and no exit
/// file has been written yet.
pub async fn is_run_command_running(ticket_key: &str, cmd_index: usize) -> bool {
    let name = workspace_container_name(ticket_key);
    let check = format!(
        "p=$(cat {pid} 2>/dev/null); [ -n \"$p\" ] && [ ! -f {exit} ] && kill -0 \"$p\" 2>/dev/null",
        pid = pid_path(cmd_index),
        exit = exit_path(cmd_index),
    );
    let output = tokio::process::Command::new("docker")
        .args(["exec", &name, "sh", "-c", &check])
        .output()
        .await;
    matches!(output, Ok(o) if o.status.success())
}

/// Read a finished run-command's exit code + log tail. Returns `None` for a
/// clean exit (code 0) or if the exit file isn't present.
async fn get_run_command_exit_error(ticket_key: &str, cmd_index: usize) -> Option<String> {
    let name = workspace_container_name(ticket_key);
    let code_out = tokio::process::Command::new("docker")
        .args([
            "exec",
            &name,
            "sh",
            "-c",
            &format!("cat {}", exit_path(cmd_index)),
        ])
        .output()
        .await
        .ok()?;
    if !code_out.status.success() {
        return None;
    }
    let exit_code: i32 = String::from_utf8_lossy(&code_out.stdout)
        .trim()
        .parse()
        .unwrap_or(0);
    if exit_code == 0 {
        return None;
    }
    let log_out = tokio::process::Command::new("docker")
        .args([
            "exec",
            &name,
            "sh",
            "-c",
            &format!("tail -n 20 {} 2>/dev/null", log_path(cmd_index)),
        ])
        .output()
        .await
        .ok();
    let log_tail = log_out
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    Some(if log_tail.is_empty() {
        format!("Command exited with code {exit_code}")
    } else {
        format!("Command exited with code {exit_code}:\n{log_tail}")
    })
}

/// Stop a run-command: signal its process group (SIGTERM) inside the workspace
/// container and clean up its runtime files. The container itself persists.
pub async fn stop_run_command(ticket_key: &str, cmd_index: usize) {
    let name = workspace_container_name(ticket_key);
    let stop = format!(
        "p=$(cat {pid} 2>/dev/null); [ -n \"$p\" ] && kill -TERM -\"$p\" 2>/dev/null; sleep 0.2; [ -n \"$p\" ] && kill -KILL -\"$p\" 2>/dev/null; rm -f {pid} {exit} {log} {script}",
        pid = pid_path(cmd_index),
        exit = exit_path(cmd_index),
        log = log_path(cmd_index),
        script = script_path(cmd_index),
    );
    let _ = tokio::process::Command::new("docker")
        .args(["exec", &name, "sh", "-c", &stop])
        .output()
        .await;
    info!(ticket = %ticket_key, cmd_index, "Run command stopped");
}

/// Best-effort cleanup of all run-commands for a ticket: kill any running
/// command process groups in the workspace container, and reap any leftover
/// legacy `takuto-run-*` containers from older versions.
pub async fn stop_all_run_commands(ticket_key: &str) {
    let name = workspace_container_name(ticket_key);
    // Kill any run-command session leaders recorded in the runtime dir.
    let kill_all = format!(
        "for f in {RUNTIME_DIR}/run-*.pid; do [ -f \"$f\" ] || continue; p=$(cat \"$f\" 2>/dev/null); [ -n \"$p\" ] && kill -KILL -\"$p\" 2>/dev/null; done; rm -f {RUNTIME_DIR}/run-*.pid {RUNTIME_DIR}/run-*.exit"
    );
    let _ = tokio::process::Command::new("docker")
        .args(["exec", &name, "sh", "-c", &kill_all])
        .output()
        .await;

    // Legacy: reap any pre-existing per-command containers.
    let sanitized = sanitize_ticket_key(ticket_key);
    let prefix = format!("takuto-run-{sanitized}-");
    if let Ok(out) = tokio::process::Command::new("docker")
        .args([
            "ps",
            "-a",
            "--filter",
            &format!("name={prefix}"),
            "--format",
            "{{.Names}}",
        ])
        .output()
        .await
        && out.status.success()
    {
        for leftover in String::from_utf8_lossy(&out.stdout).lines() {
            let leftover = leftover.trim();
            if !leftover.is_empty() {
                let _ = tokio::process::Command::new("docker")
                    .args(["rm", "-f", leftover])
                    .output()
                    .await;
            }
        }
    }
}

/// Port scanner for a run-command. Watches the workspace container for ports
/// that this command's process group opens and socat-forwards them to spare
/// host ports. Emits `run_command_port_forwarded` / `_unforwarded` events and a
/// `run_command_stopped` event (with the exit error) when the command finishes.
///
/// `owner_user_id` is the workflow owner's id; the WS layer filters events per-user.
pub async fn run_run_command_port_scanner(
    ticket_key: &str,
    cmd_index: usize,
    spare_ports: Vec<u16>,
    event_tx: broadcast::Sender<WorkflowEvent>,
    cancel: CancellationToken,
    owner_user_id: Option<String>,
) {
    let container = workspace_container_name(ticket_key);
    let ticket = ticket_key.to_string();

    let always_ignore: std::collections::HashSet<u16> = spare_ports.iter().copied().collect();

    // Baseline: ports already listening in the workspace container (the IDE,
    // other run-commands, infrastructure). Only ports that appear AFTER this
    // command starts are treated as belonging to it.
    let baseline: std::collections::HashSet<u16> =
        match scan_listening_ports(&container, true).await {
            Some(ports) => ports.into_iter().map(|(p, _)| p).collect(),
            None => std::collections::HashSet::new(),
        };

    let mut active_forwards: HashMap<u16, u16> = HashMap::new();
    let mut available_spares: Vec<u16> = spare_ports;
    let mut dirty = false;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                for (&_detected, &spare) in &active_forwards {
                    kill_socat(&container, spare).await;
                }
                remove_service_file(&container, cmd_index).await;
                info!(ticket = %ticket, cmd_index, "Run command port scanner stopped");
                return;
            }
            _ = tokio::time::sleep(std::time::Duration::from_secs(3)) => {}
        }

        if !is_run_command_running(&ticket, cmd_index).await {
            let error_msg = get_run_command_exit_error(&ticket, cmd_index).await;
            if let Some(ref err) = error_msg {
                warn!(ticket = %ticket, cmd_index, error = %err, "Run command exited with error");
            } else {
                info!(ticket = %ticket, cmd_index, "Run command finished — stopping scanner");
            }
            for (&_detected, &spare) in &active_forwards {
                kill_socat(&container, spare).await;
            }
            remove_service_file(&container, cmd_index).await;
            let _ = event_tx.send(WorkflowEvent {
                event_type: "run_command_stopped".to_string(),
                ticket_key: ticket.clone(),
                timestamp: chrono::Utc::now(),
                error: error_msg,
                step_name: Some(format!("{cmd_index}")),
                user_id: owner_user_id.clone(),
                ..Default::default()
            });
            return;
        }

        let listening = match scan_listening_ports(&container, true).await {
            Some(ports) => ports,
            None => continue,
        };
        let listening_set: std::collections::HashSet<u16> =
            listening.iter().map(|(p, _)| *p).collect();

        for &(port, family) in &listening {
            if always_ignore.contains(&port)
                || baseline.contains(&port)
                || (EDITOR_PORT_MIN..=EDITOR_PORT_MAX).contains(&port)
                || active_forwards.contains_key(&port)
            {
                continue;
            }
            if let Some(spare) = available_spares.pop() {
                if start_socat(&container, spare, port, family).await {
                    info!(ticket = %ticket, cmd_index, detected = port, host_port = spare, "Run command: dynamic port forwarded");
                    active_forwards.insert(port, spare);
                    dirty = true;
                    let host_spare = editor_host_port(spare);
                    let _ = event_tx.send(WorkflowEvent {
                        event_type: "run_command_port_forwarded".to_string(),
                        ticket_key: ticket.clone(),
                        timestamp: chrono::Utc::now(),
                        step_name: Some(format!("{cmd_index}")),
                        forwarded_port: Some((port, host_spare)),
                        user_id: owner_user_id.clone(),
                        ..Default::default()
                    });
                } else {
                    available_spares.push(spare);
                }
            }
        }

        let gone: Vec<u16> = active_forwards
            .keys()
            .copied()
            .filter(|p| !listening_set.contains(p))
            .collect();
        for port in gone {
            if let Some(spare) = active_forwards.remove(&port) {
                kill_socat(&container, spare).await;
                available_spares.push(spare);
                dirty = true;
                let host_spare = editor_host_port(spare);
                let _ = event_tx.send(WorkflowEvent {
                    event_type: "run_command_port_unforwarded".to_string(),
                    ticket_key: ticket.clone(),
                    timestamp: chrono::Utc::now(),
                    step_name: Some(format!("{cmd_index}")),
                    forwarded_port: Some((port, host_spare)),
                    user_id: owner_user_id.clone(),
                    ..Default::default()
                });
            }
        }

        // Publish this command's forwards to its service registry file so
        // sibling commands / tests can discover them.
        if dirty {
            let _ = write_file_in_container(
                &container,
                &service_path(cmd_index),
                &service_json(cmd_index, &active_forwards),
            )
            .await;
            dirty = false;
        }
    }
}

/// Best-effort removal of a command's `service-<idx>.json`.
async fn remove_service_file(container: &str, cmd_index: usize) {
    let _ = tokio::process::Command::new("docker")
        .args(["exec", container, "rm", "-f", &service_path(cmd_index)])
        .output()
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn runtime_paths_are_indexed() {
        assert_eq!(pid_path(2), "/home/takuto/.takuto-ws-runtime/run-2.pid");
        assert_eq!(exit_path(0), "/home/takuto/.takuto-ws-runtime/run-0.exit");
        assert_eq!(
            service_path(1),
            "/home/takuto/.takuto-ws-runtime/service-1.json"
        );
    }

    #[test]
    fn service_json_lists_sorted_forwards_with_urls() {
        let mut fwd = HashMap::new();
        fwd.insert(4000u16, 9105u16); // detected → spare(host)
        fwd.insert(3000u16, 9104u16);
        let json: serde_json::Value = serde_json::from_str(&service_json(2, &fwd)).unwrap();
        assert_eq!(json["command_index"], 2);
        let services = json["services"].as_array().unwrap();
        // Sorted by container_port ascending.
        assert_eq!(services[0]["container_port"], 3000);
        assert_eq!(services[0]["host_port"], 9104);
        assert_eq!(services[0]["url"], "http://localhost:3000");
        assert_eq!(services[1]["container_port"], 4000);
    }

    #[test]
    fn build_command_script_records_pid_sources_env_and_writes_exit() {
        let s = build_command_script(
            &PathBuf::from("/workspace/worktrees/proj-1"),
            "npm run dev",
            &[("TAKUTO_PROXY_BASE", "/s/abc")],
            3,
        );
        assert!(s.starts_with("#!/bin/bash"));
        // Records its own pid (session leader) and exit code, keyed by index.
        assert!(s.contains("echo $$ > /home/takuto/.takuto-ws-runtime/run-3.pid"));
        assert!(s.contains("echo $? > /home/takuto/.takuto-ws-runtime/run-3.exit"));
        assert!(s.contains("cd /workspace/worktrees/proj-1"));
        assert!(s.contains(". /etc/takuto/env"));
        assert!(s.contains("export TAKUTO_PROXY_BASE=/s/abc"));
        assert!(s.contains("npm run dev"));
    }
}
