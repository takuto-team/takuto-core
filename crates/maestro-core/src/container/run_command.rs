// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! User-defined run-command containers (`[[run_commands]]` config) — one
//! container per step, with a dedicated port scanner that forwards
//! detected dev-server ports to pre-allocated spare hosts ports.

use std::collections::HashMap;
use std::path::Path;

use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::workflow::engine::WorkflowEvent;

use super::editor::{
    EDITOR_PORT_MAX, EDITOR_PORT_MIN, allocate_editor_ports, release_container_ports,
    session_publish_arg,
};
use super::editor_host_port;
use super::port_scanner::{kill_socat, scan_listening_ports, start_socat};
use super::runner::{
    PASSTHROUGH_ENV, WORKER_ENV, apply_secrets_bundle_to_args, build_volume_args,
    passthrough_is_bundled,
};
use super::{is_dind_mode, sanitize_ticket_key};

/// Deterministic container name for a run command instance.
pub fn run_command_container_name(ticket_key: &str, cmd_index: usize) -> String {
    format!(
        "maestro-run-{}-{}",
        sanitize_ticket_key(ticket_key),
        cmd_index
    )
}

/// Information about a running run-command container.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RunCommandInfo {
    /// Index of the command in the `[[run_commands]]` config array.
    pub index: usize,
    /// The host port on which the detected application port is forwarded, if any.
    pub forwarded_port: Option<(u16, u16)>,
}

/// Start a run-command container for a workflow.
///
/// Runs the given shell `command` in a dedicated Docker container. Allocates spare
/// ports from the editor range for dynamic port forwarding (socat). Returns the
/// allocated spare ports so the caller can start a port scanner.
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
    // Phase 2b.3.x: optional per-workflow secrets bundle. Same semantics as
    // `start_editor`'s `secrets_bundle` — when `Some`, tokens reach the
    // run-command container via tmpfs file mount, never `docker inspect`.
    secrets_bundle: Option<&crate::auth::WorkerSecretsBundle>,
) -> std::result::Result<Vec<u16>, String> {
    let name = run_command_container_name(ticket_key, cmd_index);

    // Check if already running
    if is_run_command_running(ticket_key, cmd_index).await {
        return Err(format!("Run command container '{name}' is already running"));
    }

    // Remove any leftover stopped container with the same name.
    let _ = tokio::process::Command::new("docker")
        .args(["rm", "-f", &name])
        .output()
        .await;

    // Allocate spare ports for dynamic port forwarding (socat).
    let count = dynamic_ports.max(1);
    let spare_ports = allocate_editor_ports(count).await.ok_or_else(|| {
        format!(
            "No free ports available for run command (range {EDITOR_PORT_MIN}–{EDITOR_PORT_MAX})"
        )
    })?;

    info!(
        ticket = %ticket_key,
        cmd_index,
        container = %name,
        port = spare_ports[0],
        "Starting run command container"
    );

    let mut args: Vec<String> = vec![
        "run".into(),
        "-d".into(),
        // No --rm: the scanner captures exit code + logs before explicit cleanup.
        "--name".into(),
        name.clone(),
        "--cap-add=NET_ADMIN".into(),
    ];

    // Environment
    for (k, v) in WORKER_ENV {
        args.push("-e".into());
        args.push(format!("{k}={v}"));
    }
    let bundle_attached = secrets_bundle.is_some();
    for key in PASSTHROUGH_ENV {
        if bundle_attached && passthrough_is_bundled(key) {
            // Phase 2b.3.x: the bundle owns this secret; suppress the
            // ambient host value so `docker inspect` cannot leak it.
            continue;
        }
        if let Ok(val) = std::env::var(key)
            && !val.is_empty()
        {
            args.push("-e".into());
            args.push(format!("{key}={val}"));
        }
    }

    // Caller-provided environment (e.g. MAESTRO_PROXY_BASE for dev servers).
    for (k, v) in extra_env {
        args.push("-e".into());
        args.push(format!("{k}={v}"));
    }

    // Volumes — use per-issue isolation when enabled
    for mount in build_volume_args(worktree_path, isolate_workspace) {
        args.push("-v".into());
        args.push(mount);
    }

    // Phase 2b.3.x: attach the bundle mount + non-secret env vars.
    if let Some(b) = secrets_bundle {
        apply_secrets_bundle_to_args(&mut args, b);
    }

    // Port mappings — see start_editor() for the DinD vs local rationale.
    if is_dind_mode() {
        args.push("--network=host".into());
    } else {
        for &sp in &spare_ports {
            args.push("-p".into());
            args.push(session_publish_arg(sp, sp));
        }
    }

    // Working directory
    args.push("-w".into());
    args.push(worktree_path.to_string_lossy().into_owned());

    // User
    args.push("--user".into());
    args.push("maestro:maestro".into());

    // Entrypoint override — run as user directly
    args.push("--entrypoint".into());
    args.push("".into());

    // Label to identify run-command containers
    args.push("--label".into());
    args.push(format!("maestro.run_command={cmd_index}"));
    args.push("--label".into());
    args.push(format!("maestro.ticket_key={ticket_key}"));
    if !spare_ports.is_empty() {
        args.push("--label".into());
        let sp_csv: String = spare_ports
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join(",");
        args.push(format!("maestro.spare_ports={sp_csv}"));
    }

    // Image
    args.push(image.into());

    // Shell command: source env, then run the user command.
    // No `exec` prefix — the command may use shell builtins like `cd`.
    let script =
        format!("[ -f /etc/maestro/env ] && set -a && . /etc/maestro/env && set +a; {command}");
    args.push("bash".into());
    args.push("-lc".into());
    args.push(script);

    let output = tokio::process::Command::new("docker")
        .args(&args)
        .output()
        .await
        .map_err(|e| format!("Failed to start run command container: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "docker run failed for run command '{name}': {stderr}"
        ));
    }

    info!(
        ticket = %ticket_key,
        container = %name,
        "Run command container started"
    );

    Ok(spare_ports)
}

/// Check if a run-command container is currently running.
pub async fn is_run_command_running(ticket_key: &str, cmd_index: usize) -> bool {
    let name = run_command_container_name(ticket_key, cmd_index);
    let output = tokio::process::Command::new("docker")
        .args(["inspect", "--format", "{{.State.Running}}", &name])
        .output()
        .await;

    match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim() == "true",
        _ => false,
    }
}

/// Try to get the exit code and last log lines from a run-command container that
/// has exited. Returns `None` for a clean exit (code 0) or if the container was
/// already removed (`--rm`). Returns an error message string for non-zero exits.
async fn get_run_command_exit_error(container_name: &str) -> Option<String> {
    // Try docker inspect for exit code — may fail if container already removed by --rm.
    let inspect = tokio::process::Command::new("docker")
        .args(["inspect", "--format", "{{.State.ExitCode}}", container_name])
        .output()
        .await
        .ok()?;

    if !inspect.status.success() {
        // Container already removed — try to get logs from docker events or just report unknown.
        // With --rm, the container is gone. We can't retrieve logs.
        return None;
    }

    let exit_code: i32 = String::from_utf8_lossy(&inspect.stdout)
        .trim()
        .parse()
        .unwrap_or(0);

    if exit_code == 0 {
        return None;
    }

    // Container still exists momentarily — grab last few log lines before --rm cleans up.
    let logs = tokio::process::Command::new("docker")
        .args(["logs", "--tail", "20", container_name])
        .output()
        .await
        .ok();

    let log_tail = logs
        .map(|o| {
            let mut out = String::from_utf8_lossy(&o.stdout).to_string();
            let err = String::from_utf8_lossy(&o.stderr);
            if !err.trim().is_empty() {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(&err);
            }
            out.trim().to_string()
        })
        .unwrap_or_default();

    let msg = if log_tail.is_empty() {
        format!("Command exited with code {exit_code}")
    } else {
        format!("Command exited with code {exit_code}:\n{log_tail}")
    };

    Some(msg)
}

/// Stop and remove a run-command container.
pub async fn stop_run_command(ticket_key: &str, cmd_index: usize) {
    let name = run_command_container_name(ticket_key, cmd_index);
    release_container_ports(&name).await;
    let _ = tokio::process::Command::new("docker")
        .args(["rm", "-f", &name])
        .output()
        .await;
    info!(ticket = %ticket_key, cmd_index, container = %name, "Run command container stopped");
}

/// Stop ALL run-command containers for a ticket.
pub async fn stop_all_run_commands(ticket_key: &str) {
    let sanitized = sanitize_ticket_key(ticket_key);
    let prefix = format!("maestro-run-{sanitized}-");
    let output = tokio::process::Command::new("docker")
        .args([
            "ps",
            "-a",
            "--filter",
            &format!("name={prefix}"),
            "--format",
            "{{.Names}}",
        ])
        .output()
        .await;

    if let Ok(out) = output
        && out.status.success()
    {
        let stdout = String::from_utf8_lossy(&out.stdout);
        for name in stdout.lines() {
            let name = name.trim();
            if !name.is_empty() {
                let _ = tokio::process::Command::new("docker")
                    .args(["rm", "-f", name])
                    .output()
                    .await;
                info!(container = %name, "Run command container cleaned up");
            }
        }
    }
}

/// Run a port scanner for a run-command container. Similar to `run_port_scanner` for
/// editor containers, but tracks only a single container and uses its own spare ports.
/// `owner_user_id` is the workflow owner's user_id; the WS layer uses it to
/// filter scanner events per-user.
pub async fn run_run_command_port_scanner(
    ticket_key: &str,
    cmd_index: usize,
    spare_ports: Vec<u16>,
    event_tx: broadcast::Sender<WorkflowEvent>,
    cancel: CancellationToken,
    owner_user_id: Option<String>,
) {
    let container = run_command_container_name(ticket_key, cmd_index);
    let ticket = ticket_key.to_string();

    // Ports to never treat as "new": all pre-allocated spare ports.
    let mut always_ignore: std::collections::HashSet<u16> = std::collections::HashSet::new();
    for sp in &spare_ports {
        always_ignore.insert(*sp);
    }

    // Baseline: ports already listening when the scanner starts. Any port
    // present before the user's command runs is infrastructure (Docker daemon,
    // Maestro dashboard, etc.) and must not be forwarded. Only ports that
    // appear AFTER this snapshot are treated as user applications.
    let baseline = match scan_listening_ports(&container, false).await {
        Some(ports) => ports
            .into_iter()
            .map(|(p, _)| p)
            .collect::<std::collections::HashSet<u16>>(),
        None => std::collections::HashSet::new(),
    };
    if !baseline.is_empty() {
        info!(
            ticket = %ticket,
            cmd_index,
            baseline_ports = ?baseline,
            "Run command port scanner baseline captured"
        );
    }

    let mut active_forwards: HashMap<u16, u16> = HashMap::new();
    let mut available_spares: Vec<u16> = spare_ports;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                for (&detected, &spare) in &active_forwards {
                    kill_socat(&container, spare).await;
                    tracing::debug!(ticket = %ticket, cmd_index, detected, spare, "Cleaned up run-cmd socat on scanner shutdown");
                }
                info!(ticket = %ticket, cmd_index, "Run command port scanner stopped");
                return;
            }
            _ = tokio::time::sleep(std::time::Duration::from_secs(3)) => {}
        }

        // Check if the container is still running; if not, capture exit info and emit stopped event.
        if !is_run_command_running(&ticket, cmd_index).await {
            // Try to get the exit code and last log lines from the (possibly already removed) container.
            let error_msg = get_run_command_exit_error(&container).await;
            if let Some(ref err) = error_msg {
                warn!(ticket = %ticket, cmd_index, error = %err, "Run command container exited with error");
            } else {
                info!(ticket = %ticket, cmd_index, "Run command container exited — stopping scanner");
            }
            let _ = event_tx.send(WorkflowEvent {
                event_type: "run_command_stopped".to_string(),
                workflow_id: String::new(),
                ticket_key: ticket.clone(),
                state: String::new(),
                timestamp: chrono::Utc::now(),
                error: error_msg,
                step_name: Some(format!("{cmd_index}")),
                output_line: None,
                stream: None,
                progress_percent: None,
                progress_steps_total: None,
                forwarded_port: None,
                pr_merged: None,
                user_id: owner_user_id.clone(),
                ..Default::default()
            });
            // Clean up the stopped container (we removed --rm to capture exit info).
            let _ = tokio::process::Command::new("docker")
                .args(["rm", "-f", &container])
                .output()
                .await;
            return;
        }

        // owned_only=true: only detect ports with a PID visible in this
        // container's PID namespace. Prevents picking up ports from other
        // run command containers that share --network=host.
        let listening = match scan_listening_ports(&container, true).await {
            Some(ports) => ports,
            None => continue,
        };
        let listening_set: std::collections::HashSet<u16> =
            listening.iter().map(|(p, _)| *p).collect();

        // Detect new listening ports → start socat forwarding.
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
                    info!(
                        ticket = %ticket,
                        cmd_index,
                        detected = port,
                        host_port = spare,
                        "Run command: dynamic port forwarded via socat"
                    );
                    active_forwards.insert(port, spare);
                    let host_spare = editor_host_port(spare);
                    let _ = event_tx.send(WorkflowEvent {
                        event_type: "run_command_port_forwarded".to_string(),
                        workflow_id: String::new(),
                        ticket_key: ticket.clone(),
                        state: String::new(),
                        timestamp: chrono::Utc::now(),
                        error: None,
                        step_name: Some(format!("{cmd_index}")),
                        output_line: None,
                        stream: None,
                        progress_percent: None,
                        progress_steps_total: None,
                        forwarded_port: Some((port, host_spare)),
                        pr_merged: None,
                        user_id: owner_user_id.clone(),
                        ..Default::default()
                    });
                } else {
                    available_spares.push(spare);
                }
            }
        }

        // Detect removed ports → tear down socat.
        let gone: Vec<u16> = active_forwards
            .keys()
            .copied()
            .filter(|p| !listening_set.contains(p))
            .collect();

        for port in gone {
            if let Some(spare) = active_forwards.remove(&port) {
                kill_socat(&container, spare).await;
                available_spares.push(spare);
                let host_spare = editor_host_port(spare);
                let _ = event_tx.send(WorkflowEvent {
                    event_type: "run_command_port_unforwarded".to_string(),
                    workflow_id: String::new(),
                    ticket_key: ticket.clone(),
                    state: String::new(),
                    timestamp: chrono::Utc::now(),
                    error: None,
                    step_name: Some(format!("{cmd_index}")),
                    output_line: None,
                    stream: None,
                    progress_percent: None,
                    progress_steps_total: None,
                    forwarded_port: Some((port, host_spare)),
                    pr_merged: None,
                    user_id: owner_user_id.clone(),
                    ..Default::default()
                });
            }
        }
    }
}
