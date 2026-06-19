// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Editor port allocator + Docker-side port discovery. Shares the same
//! 9100–9200 range with `run-command` containers so collisions are
//! impossible across both families.

/// Port range reserved for editor instances (VS Code + app ports) on the DinD host.
pub(crate) const EDITOR_PORT_MIN: u16 = 9100;
pub(crate) const EDITOR_PORT_MAX: u16 = 19100;

/// In-memory set of allocated ports. Eliminates the race condition where
/// two concurrent `allocate_editor_ports` calls both query Docker labels
/// before either's container has started, causing both to get the same ports.
static ALLOCATED_PORTS: std::sync::LazyLock<tokio::sync::Mutex<std::collections::HashSet<u16>>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(std::collections::HashSet::new()));

/// List host ports already claimed by any Takuto-managed container
/// (`takuto-editor-*` and `takuto-run-*`). Both container families
/// allocate from the same editor port range (9100–9200), so the
/// allocator must see ports from both to avoid collisions.
async fn used_editor_ports() -> Vec<u16> {
    let mut ports = Vec::new();
    // Check both editor and run-command containers that share the port range.
    // In --network=host mode (DinD), Docker's {{.Ports}} field is empty, so we
    // also read the takuto.vscode_port and takuto.spare_ports labels which are
    // always set on editor/run-command containers.
    for filter in ["name=takuto-ws-", "name=takuto-run-"] {
        // Method 1: Docker port mappings (works in local-Docker mode).
        let output = tokio::process::Command::new("docker")
            .args(["ps", "--filter", filter, "--format", "{{.Ports}}"])
            .output()
            .await;

        if let Ok(out) = &output
            && out.status.success()
        {
            let stdout = String::from_utf8_lossy(&out.stdout);
            for segment in stdout.split([',', '\n']) {
                let segment = segment.trim();
                if let Some(arrow) = segment.find("->")
                    && let Some(colon) = segment[..arrow].rfind(':')
                {
                    let host_part = &segment[colon + 1..arrow];
                    if let Some((lo, hi)) = host_part.split_once('-') {
                        if let (Ok(lo), Ok(hi)) = (lo.parse::<u16>(), hi.parse::<u16>()) {
                            for p in lo..=hi {
                                ports.push(p);
                            }
                        }
                    } else if let Ok(p) = host_part.parse::<u16>() {
                        ports.push(p);
                    }
                }
            }
        }

        // Method 2: Docker labels (works in --network=host / DinD mode).
        let label_output = tokio::process::Command::new("docker")
            .args([
                "ps",
                "--filter",
                filter,
                "--format",
                "{{.Label \"takuto.vscode_port\"}} {{.Label \"takuto.spare_ports\"}}",
            ])
            .output()
            .await;

        if let Ok(out) = &label_output
            && out.status.success()
        {
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                for part in line.split_whitespace() {
                    // vscode_port is a single number, spare_ports is "9101,9102,..."
                    for token in part.split(',') {
                        if let Ok(p) = token.trim().parse::<u16>() {
                            ports.push(p);
                        }
                    }
                }
            }
        }
    }
    ports.sort();
    ports.dedup();
    ports
}

/// Allocate `count` free host ports from the editor range.
///
/// Uses an in-memory set (no Docker label race) plus a Docker query as a
/// secondary check for ports allocated by a previous Takuto process.
pub(crate) async fn allocate_editor_ports(count: usize) -> Option<Vec<u16>> {
    let mut allocated = ALLOCATED_PORTS.lock().await;
    // Also check Docker for ports from a previous process (restart recovery).
    let docker_used = used_editor_ports().await;
    let mut free = Vec::new();
    for p in EDITOR_PORT_MIN..=EDITOR_PORT_MAX {
        if !allocated.contains(&p) && !docker_used.contains(&p) {
            free.push(p);
            if free.len() == count {
                // Mark as allocated before returning — no race possible
                // since we hold the lock.
                for &port in &free {
                    allocated.insert(port);
                }
                return Some(free);
            }
        }
    }
    None // not enough free ports
}

/// Release ports back to the pool when a container is stopped.
pub async fn release_editor_ports(ports: &[u16]) {
    let mut allocated = ALLOCATED_PORTS.lock().await;
    for &p in ports {
        allocated.remove(&p);
    }
}

/// Allocate a single free port from the editor/terminal port range.
/// Public so the web layer can allocate terminal ports independently.
pub async fn allocate_single_port() -> Option<u16> {
    allocate_editor_ports(1).await.map(|v| v[0])
}

/// Query a container's `takuto.vscode_port` and `takuto.spare_ports`
/// labels and release them from the in-memory allocation set.
pub(crate) async fn release_container_ports(container_name: &str) {
    let output = tokio::process::Command::new("docker")
        .args([
            "inspect",
            "--format",
            "{{index .Config.Labels \"takuto.vscode_port\"}} {{index .Config.Labels \"takuto.spare_ports\"}}",
            container_name,
        ])
        .output()
        .await;
    if let Ok(out) = output
        && out.status.success()
    {
        let mut ports = Vec::new();
        for part in String::from_utf8_lossy(&out.stdout).split_whitespace() {
            for token in part.split(',') {
                if let Ok(p) = token.trim().parse::<u16>() {
                    ports.push(p);
                }
            }
        }
        if !ports.is_empty() {
            release_editor_ports(&ports).await;
        }
    }
}
