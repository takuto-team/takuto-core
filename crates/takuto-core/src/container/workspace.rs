// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Per-item **workspace container** — one long-lived container per board work
//! item that the IDE (openvscode-server), the web terminal (ttyd), and
//! user run-commands all `docker exec` into. Its PID 1 is `sleep infinity`,
//! so the container exists independently of whether the IDE is open.
//!
//! Created lazily on the first interactive operation via
//! [`ensure_workspace_container`] (idempotent + race-safe per ticket), and
//! removed when the item leaves the dashboard via
//! [`remove_workspace_container`].
//!
//! Workflow **agent steps** are intentionally *not* routed here — they keep
//! using ephemeral `takuto-worker-*` containers (see [`super::runner`]).

use std::collections::HashMap;
use std::path::Path;
use std::sync::LazyLock;

use tokio::sync::Mutex;
use tracing::{info, warn};

use super::runner::{
    PASSTHROUGH_ENV, WORKER_ENV, apply_secrets_bundle_to_args, build_volume_args,
    passthrough_is_bundled,
};
use super::sanitize_ticket_key;
use super::{editor::session_publish_arg, is_dind_mode};

/// Shared scratch directory inside the workspace container where running
/// services publish their (often randomly-assigned) URLs for other execs —
/// a sibling run-command, a test, or the agent — to read. Populated by the
/// port scanner (`services.json`) and writable by user commands.
pub const RUNTIME_DIR: &str = "/home/takuto/.takuto-ws-runtime";

/// Per-ticket async locks serialising create/start so two concurrent
/// `ensure_workspace_container` calls for the same item don't both `docker
/// run`. Mirrors the `editor::port_alloc::ALLOCATED_PORTS` pattern.
static WS_LOCKS: LazyLock<Mutex<HashMap<String, std::sync::Arc<Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

async fn ws_lock_for(ticket_key: &str) -> std::sync::Arc<Mutex<()>> {
    let mut map = WS_LOCKS.lock().await;
    map.entry(ticket_key.to_string())
        .or_insert_with(|| std::sync::Arc::new(Mutex::new(())))
        .clone()
}

/// Deterministic workspace-container name for a ticket.
pub fn workspace_container_name(ticket_key: &str) -> String {
    format!("takuto-ws-{}", sanitize_ticket_key(ticket_key))
}

/// Liveness of an item's workspace container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceStatus {
    /// No container with this name exists.
    Absent,
    /// The container exists but is not running.
    Stopped,
    /// The container exists and is running.
    Running,
}

/// Result of [`ensure_workspace_container`].
#[derive(Debug, Clone)]
pub struct WorkspaceInfo {
    /// The container name (`takuto-ws-<sanitized>`).
    pub name: String,
    /// Pre-allocated spare host ports published for dynamic forwarding.
    pub spare_ports: Vec<u16>,
    /// True if this call created the container (vs. reusing/starting one).
    pub created: bool,
}

/// Parse `docker inspect -f '{{.State.Running}}'` output into a status.
/// `inspect` fails (non-zero) for a missing container — the caller maps that
/// to [`WorkspaceStatus::Absent`]; this only distinguishes running vs stopped.
fn parse_running_flag(stdout: &str) -> WorkspaceStatus {
    if stdout.trim() == "true" {
        WorkspaceStatus::Running
    } else {
        WorkspaceStatus::Stopped
    }
}

/// Current status of an item's workspace container.
pub async fn workspace_status(ticket_key: &str) -> WorkspaceStatus {
    let name = workspace_container_name(ticket_key);
    let out = tokio::process::Command::new("docker")
        .args(["inspect", "-f", "{{.State.Running}}", &name])
        .output()
        .await;
    match out {
        Ok(o) if o.status.success() => parse_running_flag(&String::from_utf8_lossy(&o.stdout)),
        _ => WorkspaceStatus::Absent,
    }
}

/// Build the `docker run` argument vector for a fresh workspace container.
///
/// Pure (no I/O) so it is unit-testable without a Docker daemon. PID 1 is
/// `sleep infinity`; openvscode-server / ttyd / run-commands are `docker
/// exec`'d in afterwards. `dind` selects `--network=host` (DinD) vs per-port
/// loopback publishing (local), matching the editor container's model.
fn build_workspace_run_args(
    name: &str,
    image: &str,
    worktree_path: &Path,
    isolate_workspace: bool,
    spare_ports: &[u16],
    secrets_bundle: Option<&crate::auth::WorkerSecretsBundle>,
    dind: bool,
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "run".into(),
        "-d".into(),
        "--name".into(),
        name.into(),
        "--cap-add=NET_ADMIN".into(),
    ];

    // Environment (shared with worker/editor containers).
    for (k, v) in WORKER_ENV {
        args.push("-e".into());
        args.push(format!("{k}={v}"));
    }
    // Shared runtime registry directory (goal: cross-references between
    // run-commands). The directory itself is created at init time.
    args.push("-e".into());
    args.push(format!("TAKUTO_RUNTIME_DIR={RUNTIME_DIR}"));

    let bundle_attached = secrets_bundle.is_some();
    for key in PASSTHROUGH_ENV {
        if bundle_attached && passthrough_is_bundled(key) {
            continue; // the bundle owns this secret — don't leak via docker inspect
        }
        if let Ok(val) = std::env::var(key)
            && !val.is_empty()
        {
            args.push("-e".into());
            args.push(format!("{key}={val}"));
        }
    }

    // Volumes — per-issue isolation when enabled.
    for mount in build_volume_args(worktree_path, isolate_workspace) {
        args.push("-v".into());
        args.push(mount);
    }

    if let Some(b) = secrets_bundle {
        apply_secrets_bundle_to_args(&mut args, b);
    }

    // Network / port publishing — same rationale as the editor container.
    if dind {
        args.push("--network=host".into());
    } else {
        for &sp in spare_ports {
            args.push("-p".into());
            args.push(session_publish_arg(sp, sp));
        }
    }

    args.push("-w".into());
    args.push(worktree_path.to_string_lossy().into_owned());
    args.push("--user".into());
    args.push("takuto:takuto".into());
    args.push("--entrypoint".into());
    args.push("".into());

    // Marker + spare-port labels so `editor::port_alloc::used_editor_ports`
    // (extended to match `takuto-ws-`) can recover allocations after a restart.
    args.push("--label".into());
    args.push("takuto.workspace=1".into());
    if !spare_ports.is_empty() {
        let csv: String = spare_ports
            .iter()
            .map(u16::to_string)
            .collect::<Vec<_>>()
            .join(",");
        args.push("--label".into());
        args.push(format!("takuto.spare_ports={csv}"));
    }

    args.push(image.into());
    // PID 1: a do-nothing reaper-friendly process. Everything else execs in.
    args.push("sh".into());
    args.push("-c".into());
    args.push("exec sleep infinity".into());

    args
}

/// Ensure the item's single workspace container exists and is running.
///
/// Idempotent and race-safe per ticket: running → reuse; stopped → `docker
/// start`; absent → `docker run` a fresh `sleep infinity` container. Also the
/// restart-recovery point — after a Takuto restart the in-memory state is
/// empty but a still-running container is reused via `docker inspect`.
///
/// `spare_ports` are the host ports to publish (local mode) for the IDE,
/// terminal, and run-command forwarding; ignored in DinD (`--network=host`).
///
/// `init_commands` are the workspace's per-user init commands; they run inside
/// the container **whenever it is brought up** (created or started, not on
/// reuse of an already-running container), before anything uses it — so the
/// environment (e.g. a dev DB) is ready for run-commands, the terminal, and the
/// IDE alike, not just for workflow agent steps.
#[allow(clippy::too_many_arguments)]
pub async fn ensure_workspace_container(
    ticket_key: &str,
    worktree_path: &Path,
    image: &str,
    isolate_workspace: bool,
    secrets_bundle: Option<&crate::auth::WorkerSecretsBundle>,
    spare_ports: &[u16],
    init_commands: &[String],
) -> Result<WorkspaceInfo, String> {
    let lock = ws_lock_for(ticket_key).await;
    let _guard = lock.lock().await;

    let name = workspace_container_name(ticket_key);

    match workspace_status(ticket_key).await {
        WorkspaceStatus::Running => {
            return Ok(WorkspaceInfo {
                name,
                spare_ports: spare_ports.to_vec(),
                created: false,
            });
        }
        WorkspaceStatus::Stopped => {
            let out = tokio::process::Command::new("docker")
                .args(["start", &name])
                .output()
                .await
                .map_err(|e| format!("Failed to start workspace container: {e}"))?;
            if !out.status.success() {
                return Err(format!(
                    "docker start failed: {}",
                    String::from_utf8_lossy(&out.stderr)
                ));
            }
            // Re-run init on every bring-up: external side-effects (e.g. a dev
            // DB container) may have stopped while this one was down.
            run_init_commands(&name, worktree_path, init_commands).await;
            return Ok(WorkspaceInfo {
                name,
                spare_ports: spare_ports.to_vec(),
                created: false,
            });
        }
        WorkspaceStatus::Absent => {}
    }

    let args = build_workspace_run_args(
        &name,
        image,
        worktree_path,
        isolate_workspace,
        spare_ports,
        secrets_bundle,
        is_dind_mode(),
    );
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    info!(name = %name, "Creating workspace container");
    let out = tokio::process::Command::new("docker")
        .args(&arg_refs)
        .output()
        .await
        .map_err(|e| format!("Failed to create workspace container: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "docker run failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }

    // Fresh container — run the workspace's init commands before it's used.
    run_init_commands(&name, worktree_path, init_commands).await;

    Ok(WorkspaceInfo {
        name,
        spare_ports: spare_ports.to_vec(),
        created: true,
    })
}

/// Run the workspace's init commands inside the container, in the worktree,
/// with `/etc/takuto/env` sourced. Best-effort: a failing command is logged and
/// the others still run (the operation that needs them surfaces its own error).
async fn run_init_commands(name: &str, worktree_path: &Path, init_commands: &[String]) {
    if init_commands.is_empty() {
        return;
    }
    let total = init_commands.len();
    let wd = worktree_path.to_string_lossy();
    for (i, cmd) in init_commands.iter().enumerate() {
        let script = format!(
            "cd {wd} 2>/dev/null || true; [ -f /etc/takuto/env ] && set -a && . /etc/takuto/env && set +a; {cmd}"
        );
        info!(name = %name, step = i + 1, total, "Running workspace init command");
        let out = tokio::process::Command::new("docker")
            .args(["exec", name, "bash", "-lc", &script])
            .output()
            .await;
        match out {
            Ok(o) if o.status.success() => {}
            Ok(o) => warn!(
                name = %name,
                cmd = %cmd,
                stderr = %String::from_utf8_lossy(&o.stderr),
                "Workspace init command failed (continuing)"
            ),
            Err(e) => {
                warn!(name = %name, cmd = %cmd, error = %e, "Workspace init command error (continuing)")
            }
        }
    }
}

/// From all `takuto-ws-*` container names, the ones NOT in `live` — i.e.
/// workspace containers whose work item is gone (deleted/mark-done while Takuto
/// was down). Pure so it is unit-testable without Docker.
fn orphan_ws_names(all: &[String], live: &std::collections::HashSet<String>) -> Vec<String> {
    all.iter()
        .filter(|n| n.starts_with("takuto-ws-") && !live.contains(*n))
        .cloned()
        .collect()
}

/// Remove `takuto-ws-*` containers that don't correspond to a live work item.
/// Called on startup after workflows are restored, to reap containers orphaned
/// by an item deletion that raced with a Takuto restart. `live` is the set of
/// workspace-container names for the currently-live work items.
pub async fn sweep_orphan_workspaces(live: &std::collections::HashSet<String>) {
    let out = tokio::process::Command::new("docker")
        .args([
            "ps",
            "-a",
            "--filter",
            "name=takuto-ws-",
            "--format",
            "{{.Names}}",
        ])
        .output()
        .await;
    let Ok(out) = out else { return };
    if !out.status.success() {
        return;
    }
    let all: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    for name in orphan_ws_names(&all, live) {
        info!(name = %name, "Reaping orphaned workspace container");
        let _ = tokio::process::Command::new("docker")
            .args(["rm", "-f", &name])
            .output()
            .await;
    }
}

/// Force-remove the item's workspace container (best-effort).
pub async fn remove_workspace_container(ticket_key: &str) {
    let name = workspace_container_name(ticket_key);
    super::editor::release_container_ports(&name).await;
    let _ = tokio::process::Command::new("docker")
        .args(["rm", "-f", &name])
        .output()
        .await;
    info!(name = %name, "Workspace container removed");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn name_is_sanitized_and_prefixed() {
        assert_eq!(workspace_container_name("PROJ-12"), "takuto-ws-proj-12");
        assert_eq!(workspace_container_name("a/b.c"), "takuto-ws-a-b-c");
    }

    #[test]
    fn orphan_ws_names_keeps_only_dead_workspaces() {
        let all = vec![
            "takuto-ws-proj-1".to_string(),
            "takuto-ws-proj-2".to_string(),
            "takuto-editor-old".to_string(), // not a ws- container → ignored
            "takuto-worker-proj-1-0".to_string(),
        ];
        let live: std::collections::HashSet<String> =
            ["takuto-ws-proj-1".to_string()].into_iter().collect();
        assert_eq!(orphan_ws_names(&all, &live), vec!["takuto-ws-proj-2"]);
    }

    #[test]
    fn running_flag_parses() {
        assert_eq!(parse_running_flag("true\n"), WorkspaceStatus::Running);
        assert_eq!(parse_running_flag(" true "), WorkspaceStatus::Running);
        assert_eq!(parse_running_flag("false"), WorkspaceStatus::Stopped);
        assert_eq!(parse_running_flag(""), WorkspaceStatus::Stopped);
    }

    #[test]
    fn run_args_local_publishes_ports_and_sleeps() {
        let args = build_workspace_run_args(
            "takuto-ws-proj-1",
            "takuto:latest",
            &PathBuf::from("/workspace/worktrees/proj-1"),
            true,
            &[9101, 9102],
            None,
            false, // local docker
        );
        // PID 1 is sleep infinity.
        let tail = args.iter().rev().take(3).cloned().collect::<Vec<_>>();
        assert_eq!(tail, vec!["exec sleep infinity", "-c", "sh"]);
        // Image precedes the command.
        assert!(args.contains(&"takuto:latest".to_string()));
        // Local mode publishes each spare port, no host network.
        assert!(args.iter().any(|a| a == "-p"));
        assert!(args.iter().any(|a| a.contains("9101")));
        assert!(!args.iter().any(|a| a == "--network=host"));
        // Working dir + workspace marker label.
        assert!(
            args.windows(2)
                .any(|w| w[0] == "-w" && w[1] == "/workspace/worktrees/proj-1")
        );
        assert!(args.iter().any(|a| a == "takuto.workspace=1"));
        assert!(args.iter().any(|a| a == "takuto.spare_ports=9101,9102"));
        // Runtime registry env exported.
        assert!(
            args.iter()
                .any(|a| a == &format!("TAKUTO_RUNTIME_DIR={RUNTIME_DIR}"))
        );
    }

    #[test]
    fn run_args_dind_uses_host_network_no_publish() {
        let args = build_workspace_run_args(
            "takuto-ws-proj-1",
            "takuto:latest",
            &PathBuf::from("/workspace/worktrees/proj-1"),
            true,
            &[9101],
            None,
            true, // dind
        );
        assert!(args.iter().any(|a| a == "--network=host"));
        assert!(!args.iter().any(|a| a == "-p"));
    }
}
