// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Background port scanner that detects ports listening inside an editor
//! container and socat-forwards them to pre-allocated spare host ports.

use std::collections::HashMap;

use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::workflow::engine::WorkflowEvent;

use super::editor::{EDITOR_PORT_MAX, EDITOR_PORT_MIN, editor_container_name};
use super::editor_host_port;

/// Scan listening ports inside an editor container and socat-forward them to spare
/// host ports. Works for apps binding to either 0.0.0.0 or 127.0.0.1 inside the
/// container, because socat runs inside the container and connects via `localhost`.
///
/// `spare_ports` is the pool of symmetric-mapped host ports available for forwarding.
/// `owner_user_id` is the workflow owner's user_id; the WS layer uses it to
/// filter scanner events per-user.
pub async fn run_port_scanner(
    ticket_key: &str,
    vscode_port: u16,
    spare_ports: Vec<u16>,
    event_tx: broadcast::Sender<WorkflowEvent>,
    cancel: CancellationToken,
    owner_user_id: Option<String>,
) {
    let container = editor_container_name(ticket_key);
    let ticket = ticket_key.to_string();

    // Ports to never treat as "new": VS Code and all pre-allocated spare ports
    // (they are docker-mapped; socat may briefly keep them LISTENing after kill,
    // so never re-forward them).
    let mut always_ignore: std::collections::HashSet<u16> = std::collections::HashSet::new();
    always_ignore.insert(vscode_port);
    for sp in &spare_ports {
        always_ignore.insert(*sp);
    }

    // Baseline: ports already listening when the scanner starts (infrastructure
    // services like the Docker daemon in DinD mode, the Takuto dashboard, etc.).
    // Only ports that appear AFTER this snapshot are treated as user applications.
    // This avoids hardcoding a blocklist — users are free to use any port.
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
            baseline_ports = ?baseline,
            "Port scanner baseline captured — these ports will be ignored"
        );
    }

    // detected_port → spare_port (the host port socat is listening on).
    let mut active_forwards: HashMap<u16, u16> = HashMap::new();
    let mut available_spares: Vec<u16> = spare_ports;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                for (&detected, &spare) in &active_forwards {
                    kill_socat(&container, spare).await;
                    debug!(ticket = %ticket, detected, spare, "Cleaned up socat on scanner shutdown");
                }
                info!(ticket = %ticket, "Port scanner stopped");
                return;
            }
            _ = tokio::time::sleep(std::time::Duration::from_secs(3)) => {}
        }

        let listening = match scan_listening_ports(&container, false).await {
            Some(ports) => ports,
            None => continue,
        };
        let listening_set: std::collections::HashSet<u16> =
            listening.iter().map(|(p, _)| *p).collect();

        // Detect new listening ports → start socat forwarding.
        for &(port, family) in &listening {
            // Skip infrastructure ports: baseline (pre-existing), always_ignore
            // (VS Code + spare ports), anything in the managed editor port range
            // (allocated for VS Code, terminals, and socat — not user apps), and
            // ports already forwarded.
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
                        detected = port,
                        host_port = spare,
                        "Dynamic port forwarded via socat"
                    );
                    active_forwards.insert(port, spare);
                    let host_spare = editor_host_port(spare);
                    let _ = event_tx.send(WorkflowEvent {
                        event_type: "port_forwarded".to_string(),
                        workflow_id: String::new(),
                        ticket_key: ticket.clone(),
                        state: String::new(),
                        timestamp: chrono::Utc::now(),
                        error: None,
                        step_name: None,
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
            } else {
                warn!(ticket = %ticket, port, "No spare ports left for dynamic forwarding");
            }
        }

        // --- Dynamic ports: detect removed, tear down socat. ---
        let gone: Vec<u16> = active_forwards
            .keys()
            .copied()
            .filter(|p| !listening_set.contains(p))
            .collect();

        for port in gone {
            if let Some(spare) = active_forwards.remove(&port) {
                kill_socat(&container, spare).await;
                // NOTE: do NOT remove `spare` from always_ignore — socat may linger
                // briefly in LISTEN state, and we don't want the next scan to treat
                // it as a new dev server. Spares always stay ignored.
                available_spares.push(spare);
                info!(
                    ticket = %ticket,
                    detected = port,
                    host_port = spare,
                    "Dynamic port forward removed"
                );

                let host_spare = editor_host_port(spare);
                let _ = event_tx.send(WorkflowEvent {
                    event_type: "port_unforwarded".to_string(),
                    workflow_id: String::new(),
                    ticket_key: ticket.clone(),
                    state: String::new(),
                    timestamp: chrono::Utc::now(),
                    error: None,
                    step_name: None,
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

/// Address family detected for a listener. Affects socat's connect side so we
/// reach apps regardless of whether they bind to 127.0.0.1 or ::1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ListenFamily {
    Ipv4,
    Ipv6,
}

/// Run `ss -tlnH` inside the container and return listening `(port, family)` entries.
/// If the same port is listening on both families, prefer IPv4 (better Docker reachability).
/// Scan listening ports inside a container.
///
/// When `owned_only` is true, uses `ss -tlnpH` (with process info) and
/// only returns ports that have a visible PID in this container's PID
/// namespace. In `--network=host` mode, ports from OTHER containers
/// show no PID — this lets each scanner see only its own app's ports,
/// preventing cross-contamination when multiple run commands share the
/// same network namespace.
pub(crate) async fn scan_listening_ports(
    container: &str,
    owned_only: bool,
) -> Option<Vec<(u16, ListenFamily)>> {
    let ss_args = if owned_only {
        vec!["ss", "-tlnpH"]
    } else {
        vec!["ss", "-tlnH"]
    };
    let output = tokio::process::Command::new("docker")
        .arg("exec")
        .arg(container)
        .args(&ss_args)
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut by_port: HashMap<u16, ListenFamily> = HashMap::new();
    // Without -p: "LISTEN 0 128 0.0.0.0:6006  0.0.0.0:*"
    // With -p:    "LISTEN 0 128 0.0.0.0:6006  0.0.0.0:*  users:(("node",pid=42,fd=19))"
    //   or no PID for other containers' ports:
    //             "LISTEN 0 128 0.0.0.0:5173  0.0.0.0:*"
    for line in stdout.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 4 {
            continue;
        }
        // When owned_only, skip lines without "users:(" — those are ports
        // from other containers (no PID visible in this PID namespace).
        if owned_only && !line.contains("users:(") {
            continue;
        }
        let local = fields[3];
        let family = if local.starts_with('[') {
            ListenFamily::Ipv6
        } else {
            ListenFamily::Ipv4
        };
        if let Some(port_str) = local.rsplit(':').next()
            && let Ok(port) = port_str.parse::<u16>()
            && port > 0
        {
            let entry = by_port.entry(port).or_insert(family);
            if family == ListenFamily::Ipv4 {
                *entry = ListenFamily::Ipv4;
            }
        }
    }
    Some(by_port.into_iter().collect())
}

/// Return the set of ports currently listening inside the editor container.
/// Used by `open_terminal` to avoid picking a spare port already bound by socat.
/// Returns an empty set if the container is unreachable or `ss` fails.
pub async fn listening_ports_in_editor(ticket_key: &str) -> std::collections::HashSet<u16> {
    let name = editor_container_name(ticket_key);
    scan_listening_ports(&name, false)
        .await
        .map(|v| v.into_iter().map(|(p, _)| p).collect())
        .unwrap_or_default()
}

/// List the live `socat` forwards running inside `container`, as
/// `(spare_host_port, target_app_port)` pairs.
///
/// Parses the `socat TCP4-LISTEN:<spare>,... TCP[46]:<addr>:<target>` command
/// lines from `pgrep -af socat`. This is the ground truth for which dynamic
/// port forwards are actually active, independent of any in-memory bookkeeping
/// — used to rebuild the dashboard's port chips from the running container so
/// they survive a server restart or scanner churn.
pub async fn live_socat_forwards(container: &str) -> Vec<(u16, u16)> {
    parse_socat_forwards(&process_cmdlines(container).await)
}

/// Read every process command line inside `container` from `/proc`, one per
/// line, with NUL separators replaced by spaces.
///
/// This stands in for `pgrep`/`ps`, which are NOT installed in the workspace
/// image — `/proc` always is. Returns an empty string if the exec fails.
pub(crate) async fn process_cmdlines(container: &str) -> String {
    let out = tokio::process::Command::new("docker")
        .args([
            "exec",
            container,
            "sh",
            "-c",
            r#"for d in /proc/[0-9]*; do tr '\0' ' ' < "$d/cmdline" 2>/dev/null; echo; done"#,
        ])
        .output()
        .await;
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        _ => String::new(),
    }
}

/// Parse `(spare_host_port, target_app_port)` pairs from `pgrep -af socat`
/// output. Each forward line looks like:
/// `<pid> socat TCP4-LISTEN:<spare>,fork,reuseaddr,bind=0.0.0.0 TCP4:127.0.0.1:<target>`
/// (the target may instead be `TCP6:[::1]:<target>`).
fn parse_socat_forwards(stdout: &str) -> Vec<(u16, u16)> {
    let mut forwards = Vec::new();
    for line in stdout.lines() {
        let spare = line
            .split_whitespace()
            .find_map(|tok| tok.strip_prefix("TCP4-LISTEN:"))
            .and_then(|rest| rest.split(',').next())
            .and_then(|p| p.parse::<u16>().ok());
        // Target token: "TCP4:127.0.0.1:<port>" or "TCP6:[::1]:<port>".
        let target = line
            .split_whitespace()
            .find(|tok| tok.starts_with("TCP4:") || tok.starts_with("TCP6:"))
            .and_then(|tok| tok.rsplit(':').next())
            .and_then(|p| p.parse::<u16>().ok());
        if let (Some(spare), Some(target)) = (spare, target) {
            forwards.push((spare, target));
        }
    }
    forwards
}

/// Start a `socat` process inside the container to forward `spare_port` → `target_port`.
pub(crate) async fn start_socat(
    container: &str,
    spare_port: u16,
    target_port: u16,
    target_family: ListenFamily,
) -> bool {
    // Listen on IPv4 0.0.0.0 (Docker port-proxy connects via IPv4). For the target,
    // use the same family the app is bound on — apps that bind to ::1 (IPv6 localhost,
    // common with Node.js / Vite) are unreachable via 127.0.0.1.
    let target = match target_family {
        ListenFamily::Ipv4 => format!("TCP4:127.0.0.1:{target_port}"),
        ListenFamily::Ipv6 => format!("TCP6:[::1]:{target_port}"),
    };
    let output = tokio::process::Command::new("docker")
        .args([
            "exec",
            "-d",
            container,
            "socat",
            &format!("TCP4-LISTEN:{spare_port},fork,reuseaddr,bind=0.0.0.0"),
            &target,
        ])
        .output()
        .await;

    match output {
        Ok(o) if o.status.success() => {
            info!(container, spare_port, target_port, family = ?target_family, "socat forward started");
            true
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            warn!(container, spare_port, target_port, %stderr, "socat start failed");
            false
        }
        Err(e) => {
            warn!(container, spare_port, target_port, error = %e, "docker exec socat failed");
            false
        }
    }
}

/// Kill the `socat` process listening on `spare_port` inside the container.
pub(crate) async fn kill_socat(container: &str, spare_port: u16) {
    // Match either TCP-LISTEN or TCP4-LISTEN with the spare port.
    pkill_in_container(container, &format!("LISTEN:{spare_port}")).await;
}

/// Send SIGTERM to every process inside `container` whose `/proc` command line
/// contains `needle`.
///
/// Replaces `pkill -f`, which is NOT installed in the workspace image. The
/// match is done in-shell with a `case` glob (not `grep`) so the matcher's own
/// process never matches `needle` and kills itself. `kill` is a shell builtin,
/// so this depends only on `sh`, `tr`, and `/proc` — all present.
pub async fn pkill_in_container(container: &str, needle: &str) {
    // `needle` is built from internal constants / numeric ports, never user
    // input, so plain interpolation into the glob is safe here.
    let script = format!(
        r#"for d in /proc/[0-9]*; do cmd=$(tr '\0' ' ' < "$d/cmdline" 2>/dev/null); case "$cmd" in *{needle}*) kill "${{d#/proc/}}" 2>/dev/null ;; esac; done"#
    );
    let _ = tokio::process::Command::new("docker")
        .args(["exec", container, "sh", "-c", &script])
        .output()
        .await;
}

#[cfg(test)]
mod tests {
    use super::parse_socat_forwards;

    #[test]
    fn parses_ipv4_and_ipv6_socat_forwards() {
        let stdout = "\
42 socat TCP4-LISTEN:9110,fork,reuseaddr,bind=0.0.0.0 TCP4:127.0.0.1:5173
43 socat TCP4-LISTEN:9111,fork,reuseaddr,bind=0.0.0.0 TCP6:[::1]:6006
";
        let mut forwards = parse_socat_forwards(stdout);
        forwards.sort();
        assert_eq!(forwards, vec![(9110, 5173), (9111, 6006)]);
    }

    #[test]
    fn ignores_non_forward_lines() {
        let stdout = "\
99 socat -V
100 grep socat
";
        assert!(parse_socat_forwards(stdout).is_empty());
    }

    #[test]
    fn parses_prefixless_proc_cmdline_form() {
        // /proc/<pid>/cmdline form: NULs→spaces, no leading PID, trailing space.
        let stdout = "socat TCP4-LISTEN:9110,fork,reuseaddr,bind=0.0.0.0 TCP6:[::1]:5173 \n";
        assert_eq!(parse_socat_forwards(stdout), vec![(9110, 5173)]);
    }

    #[test]
    fn parses_multiple_forwards() {
        let stdout = "\
socat TCP4-LISTEN:9110,fork,reuseaddr,bind=0.0.0.0 TCP4:127.0.0.1:3000
socat TCP4-LISTEN:9111,fork,reuseaddr,bind=0.0.0.0 TCP4:127.0.0.1:8080
socat TCP4-LISTEN:9112,fork,reuseaddr,bind=0.0.0.0 TCP6:[::1]:5173
";
        assert_eq!(
            parse_socat_forwards(stdout),
            vec![(9110, 3000), (9111, 8080), (9112, 5173)]
        );
    }

    #[test]
    fn skips_listener_without_a_target() {
        // A malformed/partial line with only the LISTEN side yields nothing.
        assert!(parse_socat_forwards("socat TCP4-LISTEN:9110,fork\n").is_empty());
    }

    #[test]
    fn ignores_empty_input() {
        assert!(parse_socat_forwards("").is_empty());
        assert!(parse_socat_forwards("\n\n").is_empty());
    }
}
