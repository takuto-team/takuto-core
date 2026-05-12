// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::workflow::engine::WorkflowEvent;

/// Shell-escape a string for safe inclusion in `sh -c "..."`.
fn shell_escape(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    // If the string is safe (alphanumeric, common flags), return as-is.
    if s.bytes().all(|b| {
        b.is_ascii_alphanumeric()
            || b == b'-'
            || b == b'_'
            || b == b'/'
            || b == b'.'
            || b == b'='
            || b == b':'
    }) {
        return s.to_string();
    }
    // Wrap in single quotes, escaping embedded single quotes.
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Sanitize a ticket key for use in container names (lowercase, replace non-alphanumeric with `-`).
fn sanitize_ticket_key(key: &str) -> String {
    key.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

/// Return the host-visible port for an editor-range container port.
///
/// In both local-Docker and DinD modes, symmetric port mappings mean the
/// host port equals the container port. This wrapper exists so callers
/// don't embed that assumption directly.
pub fn editor_host_port(container_port: u16) -> u16 {
    container_port
}

/// Whether we are running in Docker-in-Docker mode (DOCKER_HOST is set).
fn is_dind_mode() -> bool {
    std::env::var("DOCKER_HOST").is_ok()
}

/// Runs AI agent commands inside isolated Docker containers so each workflow
/// gets its own filesystem and network namespace.
pub struct ContainerRunner {
    ticket_key: String,
    image: String,
    worktree_path: PathBuf,
    step_counter: std::sync::atomic::AtomicU32,
    /// When `true`, replace the broad `/workspace:/workspace` mount with targeted
    /// bind mounts for just the worktree path, `.git`, and `.maestro`. This prevents
    /// a container from accessing any other issue's worktree.
    isolate_workspace: bool,
}

static DOCKER_AVAILABLE: OnceLock<bool> = OnceLock::new();
/// Throttle DinD image pruning to at most once every 5 minutes.
static LAST_IMAGE_PRUNE: AtomicU64 = AtomicU64::new(0);
const IMAGE_PRUNE_INTERVAL_SECS: u64 = 300;

/// Fixed environment variables injected into every worker container.
const WORKER_ENV: &[(&str, &str)] = &[
    ("HOME", "/home/maestro"),
    ("MAESTRO_HOME", "/home/maestro"),
    ("CURSOR_CONFIG_DIR", "/home/maestro/.cursor"),
    ("MISE_DATA_DIR", "/home/maestro/.local/share/mise"),
    ("MISE_CACHE_DIR", "/home/maestro/.cache/mise"),
    ("MISE_CONFIG_DIR", "/home/maestro/.config/mise"),
    ("MISE_TRUST_ALL_CONFIGS", "1"),
    ("MISE_YES", "1"),
    (
        "PATH",
        "/home/maestro/.local/share/mise/shims:/usr/local/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
    ),
    ("MAESTRO_CONFIG", "/etc/maestro/config.toml"),
    // Persist user-level .npmrc across worker containers (aws codeartifact login writes here)
    ("NPM_CONFIG_USERCONFIG", "/workspace/.maestro/.npmrc"),
    // Deterministic text rendering in screenshots / snapshots (Playwright, Storybook, etc.)
    ("TZ", "UTC"),
    ("LANG", "C.UTF-8"),
    ("LC_ALL", "C.UTF-8"),
];

/// Host environment variables forwarded into the worker when set.
const PASSTHROUGH_ENV: &[&str] = &[
    // Claude Code auth (token + optional base URL override)
    "CLAUDE_CODE_OAUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "FIGMA_API_TOKEN",
    // figma-cli (`fcli`) personal access token; takes priority over stored auth.
    "FIGMA_ACCESS_TOKEN",
    // Lokalise CLI v2 (`lokalise2`) — the tool itself reads `--token`; exporting a
    // var lets users wrap invocations (e.g. `lokalise2 --token "$LOKALISE_API_TOKEN"`)
    // or write a thin shell alias in maestro.env.
    "LOKALISE_API_TOKEN",
    "CURSOR_API_KEY",
    // Ambient GH_TOKEN fallback for local development (no GitHub App / no token file).
    // When the centralized token file exists, workers read from that instead.
    "GH_TOKEN",
    // Optional: force a fixed browser bundle (must match the project's @playwright/test version).
    "PLAYWRIGHT_BROWSERS_PATH",
    // Match CI behaviour when needed (some tools tweak output when CI is set).
    "CI",
    // Override defaults above when the host sets them.
    "TZ",
    "LANG",
    "LC_ALL",
];

/// Volume mounts shared between the orchestrator and every worker container.
const WORKER_VOLUMES: &[&str] = &[
    "/workspace:/workspace",
    "/shared-auth/claude:/home/maestro/.claude",
    "/shared-auth/cursor:/home/maestro/.cursor",
    // npx skills add -g stores actual files in .agents/skills/; .claude/skills/ and
    // .cursor/skills/ contain symlinks pointing there, so this must be shared.
    "/shared-auth/agents:/home/maestro/.agents",
    "/shared-auth/gh:/home/maestro/.config/gh",
    "/shared-auth/acli:/home/maestro/.config/acli",
    "/shared-auth/fcli:/home/maestro/.config/fcli",
    "/shared-auth/npm:/home/maestro/.npm",
    "/shared-auth/mise-data:/home/maestro/.local/share/mise",
    "/shared-auth/mise-cache:/home/maestro/.cache/mise",
    "/shared-auth/aws:/home/maestro/.aws",
    // Playwright browser cache — must align with the repo's package.json, not a baked image path
    "/shared-auth/playwright-cache:/home/maestro/.cache/ms-playwright",
    // openvscode-server data (extensions, settings, state)
    "/shared-auth/vscode:/home/maestro/.openvscode-server",
    // Config + env for egress rules (extra_egress_hosts, .npmrc registry hosts, allow_all_https)
    "/etc/maestro:/etc/maestro:ro",
];

/// Build the list of volume mount strings for a Docker container.
///
/// When `isolate_workspace` is `true`, the broad `/workspace:/workspace` mount is
/// replaced with three targeted mounts so the container sees only:
///   - its own worktree directory (read-write)
///   - the shared `.git` internals (needed for git operations)
///   - the shared `.maestro` directory (read-only; contains `.npmrc`, etc.)
///
/// All other mounts from [`WORKER_VOLUMES`] (auth volumes, `/etc/maestro`) are preserved.
///
/// The repo root is derived as the grandparent of `worktree_path`
/// (e.g. `/workspace/worktrees/slug` → `/workspace`).
pub fn build_volume_args(worktree_path: &Path, isolate_workspace: bool) -> Vec<String> {
    let mut mounts = Vec::new();
    for v in WORKER_VOLUMES {
        if isolate_workspace && *v == "/workspace:/workspace" {
            continue;
        }
        mounts.push((*v).to_string());
    }
    if isolate_workspace {
        if let Some(repo_root) = worktree_path.parent().and_then(|p| p.parent()) {
            let wt = worktree_path.to_string_lossy();
            let root = repo_root.to_string_lossy();
            mounts.push(format!("{wt}:{wt}"));
            mounts.push(format!("{root}/.git:{root}/.git"));
            mounts.push(format!("{root}/.maestro:{root}/.maestro:ro"));
        } else {
            warn!(
                path = %worktree_path.display(),
                "Cannot derive repo root from worktree path (need grandparent); \
                 falling back to full /workspace mount"
            );
            mounts.push("/workspace:/workspace".to_string());
        }
    }
    mounts
}

impl ContainerRunner {
    pub fn new(ticket_key: &str, worktree_path: &Path, image: &str) -> Self {
        Self {
            ticket_key: ticket_key.to_string(),
            image: image.to_string(),
            worktree_path: worktree_path.to_path_buf(),
            step_counter: std::sync::atomic::AtomicU32::new(0),
            isolate_workspace: false,
        }
    }

    /// Enable per-issue workspace isolation. Instead of mounting the full
    /// `/workspace` volume, only the worktree directory, `.git`, and `.maestro`
    /// are mounted. This prevents a container from accessing other issues' files.
    pub fn with_isolate_workspace(mut self) -> Self {
        self.isolate_workspace = true;
        self
    }

    /// Check if Docker is available (`DOCKER_HOST` set and `docker info` succeeds).
    /// The result is cached for the process lifetime.
    pub fn is_available() -> bool {
        *DOCKER_AVAILABLE.get_or_init(|| {
            if std::env::var("DOCKER_HOST").unwrap_or_default().is_empty() {
                error!("DOCKER_HOST not set — DinD is required; workflows will fail");
                return false;
            }
            let ok = std::process::Command::new("docker")
                .args(["info"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if ok {
                info!("Docker daemon reachable — container isolation enabled");
            } else {
                error!("docker info failed — DinD is required; workflows will fail");
            }
            ok
        })
    }

    /// Returns a unique container name for this ticket, incrementing an internal counter.
    pub fn next_container_name(&self) -> String {
        let n = self
            .step_counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let sanitized = sanitize_ticket_key(&self.ticket_key);
        format!("maestro-worker-{sanitized}-{n}")
    }

    /// Build the common `docker run` prefix (flags, env, volumes, workdir, entrypoint)
    /// before the image name and user command.
    fn base_docker_args(&self, container_name: &str, entrypoint: Option<&str>) -> Vec<String> {
        let mut args = vec![
            "run".into(),
            "--rm".into(),
            "--name".into(),
            container_name.into(),
            "--cap-add=NET_ADMIN".into(),
        ];

        for (k, v) in WORKER_ENV {
            args.push("-e".into());
            args.push(format!("{k}={v}"));
        }

        for key in PASSTHROUGH_ENV {
            if let Ok(val) = std::env::var(key)
                && !val.is_empty()
            {
                args.push("-e".into());
                args.push(format!("{key}={val}"));
            }
        }

        for mount in build_volume_args(&self.worktree_path, self.isolate_workspace) {
            args.push("-v".into());
            args.push(mount);
        }

        args.push("-w".into());
        args.push(self.worktree_path.to_string_lossy().into_owned());

        args.push("--entrypoint".into());
        args.push(entrypoint.unwrap_or("").into());

        args
    }

    /// Wrap a direct command (`program` + `args`) into a `docker run` invocation.
    ///
    /// Uses `sh -c` so we can restore `.claude.json` from backup before exec-ing
    /// the actual program (the file lives outside the shared volume and is missing
    /// in fresh worker containers).
    pub fn wrap_command(&self, program: &str, args: &[&str]) -> (String, Vec<String>) {
        let name = self.next_container_name();
        let mut docker_args = self.base_docker_args(&name, None);
        // Without `--user`, `docker run` defaults to root and writes root-owned files on the
        // bind-mounted repo/worktree; the Maestro process (user `maestro`) cannot remove them later.
        docker_args.push("--user".into());
        docker_args.push("maestro:maestro".into());
        docker_args.push(self.image.clone());

        // Build a shell command that restores .claude.json then exec's the program.
        let mut shell_parts: Vec<String> = Vec::new();
        shell_parts.push(shell_escape(program));
        for a in args {
            shell_parts.push(shell_escape(a));
        }
        let restore = r#"if [ ! -f "$HOME/.claude.json" ]; then b=$(ls -t "$HOME/.claude/backups/.claude.json.backup."* 2>/dev/null | head -1); [ -n "$b" ] && cp "$b" "$HOME/.claude.json"; fi"#;
        // Ensure npm/mise dirs are owned by maestro (shared volumes start root-owned).
        // Uses passwordless sudo bash (granted in /etc/sudoers.d/maestro-hook-bash).
        let fix_perms = r#"sudo -n bash -c 'for d in "$HOME/.npm" "$HOME/.npm-global" "$HOME/.cache/mise" "$HOME/.local/share/mise"; do [ -d "$d" ] && chown -R "$(id -u):$(id -g)" "$d"; done' 2>/dev/null || true"#;
        // Source the centralized GitHub App token so `gh` and git operations use a
        // fresh token. The token file is refreshed by Maestro's background service.
        let gh_token = r#"[ -f "$HOME/.config/gh/gh-app-token" ] && export GH_TOKEN="$(cat "$HOME/.config/gh/gh-app-token")";"#;
        let cmd = format!(
            "{restore}; {fix_perms}; {gh_token} exec {}",
            shell_parts.join(" ")
        );
        docker_args.push("sh".into());
        docker_args.push("-c".into());
        docker_args.push(cmd);

        ("docker".into(), docker_args)
    }

    /// Wrap a shell command string into a `docker run` invocation using the worker entrypoint
    /// (egress rules + `runuser`).
    pub fn wrap_shell_command(&self, cmd: &str) -> (String, Vec<String>) {
        let name = self.next_container_name();
        let mut docker_args =
            self.base_docker_args(&name, Some("/usr/local/bin/worker-entrypoint.sh"));
        docker_args.push(self.image.clone());
        docker_args.push("sh".into());
        docker_args.push("-c".into());
        docker_args.push(cmd.into());
        ("docker".into(), docker_args)
    }

    /// Force-remove all worker containers for this ticket.
    pub async fn force_remove_all(&self) {
        let sanitized = sanitize_ticket_key(&self.ticket_key);
        remove_containers_matching(&sanitized).await;
    }

    /// Force-remove all worker containers for a given ticket key (no instance needed).
    pub async fn cleanup_for_ticket(ticket_key: &str) {
        let sanitized = sanitize_ticket_key(ticket_key);
        remove_containers_matching(&sanitized).await;
        prune_dangling_images().await;
    }

    /// Auto-detect the worker image by inspecting the running Maestro container,
    /// falling back to a locally-present `maestro:latest`, then `MAESTRO_REGISTRY_IMAGE`.
    pub async fn discover_worker_image() -> Option<String> {
        // Inspect the current container by hostname (Docker sets HOSTNAME to the
        // container ID). This works regardless of the compose project name — no
        // hardcoded container_name needed.
        let container_id = std::env::var("HOSTNAME").unwrap_or_default();
        let output = if !container_id.is_empty() {
            tokio::process::Command::new("docker")
                .args(["inspect", &container_id, "--format", "{{.Config.Image}}"])
                .output()
                .await
                .ok()
        } else {
            None
        };

        if let Some(output) = output
            && output.status.success()
        {
            let image = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !image.is_empty() {
                // Verify the image actually exists in DinD before using it — the name from
                // docker inspect may point to a registry tag (e.g. ghcr.io/…:dev) that was
                // never pulled into DinD (local dev builds).
                let exists = tokio::process::Command::new("docker")
                    .args(["image", "inspect", &image])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .await
                    .map(|s| s.success())
                    .unwrap_or(false);
                if exists {
                    info!(image = %image, "Discovered worker image from running Maestro container");
                    return Some(image);
                }
                info!(
                    image = %image,
                    "Image from docker inspect not present in DinD — trying maestro:latest"
                );
            }
        }

        // Check if maestro:latest is present locally in DinD (e.g. loaded via `make load-worker`).
        // This is the correct image for local development builds.
        let local_latest = tokio::process::Command::new("docker")
            .args(["image", "inspect", "maestro:latest"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false);
        if local_latest {
            info!("Using local maestro:latest as worker image");
            return Some("maestro:latest".to_string());
        }

        // Fall back to MAESTRO_REGISTRY_IMAGE (set at build time in the Dockerfile)
        if let Ok(image) = std::env::var("MAESTRO_REGISTRY_IMAGE")
            && !image.is_empty()
        {
            info!(image = %image, "Using MAESTRO_REGISTRY_IMAGE as worker image");
            return Some(image);
        }

        warn!(
            "Cannot auto-detect worker image — docker inspect failed, maestro:latest not found, and MAESTRO_REGISTRY_IMAGE not set"
        );
        None
    }
}

// ---------------------------------------------------------------------------
// Editor container management
// ---------------------------------------------------------------------------

/// Convert a TOML value to a serde_json value for VS Code settings.json.
fn toml_value_to_json(val: &toml::Value) -> serde_json::Value {
    match val {
        toml::Value::String(s) => serde_json::Value::String(s.clone()),
        toml::Value::Integer(i) => serde_json::json!(*i),
        toml::Value::Float(f) => serde_json::json!(*f),
        toml::Value::Boolean(b) => serde_json::Value::Bool(*b),
        toml::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(toml_value_to_json).collect())
        }
        toml::Value::Table(tbl) => {
            let map: serde_json::Map<String, serde_json::Value> = tbl
                .iter()
                .map(|(k, v)| (k.clone(), toml_value_to_json(v)))
                .collect();
            serde_json::Value::Object(map)
        }
        toml::Value::Datetime(dt) => serde_json::Value::String(dt.to_string()),
    }
}

/// Port range reserved for editor instances (VS Code + app ports) on the DinD host.
const EDITOR_PORT_MIN: u16 = 9100;
const EDITOR_PORT_MAX: u16 = 19100;

/// Information about a running editor container.
#[derive(Debug, Clone, serde::Serialize)]
pub struct EditorInfo {
    /// URL to open in the browser (e.g. `http://localhost:9100/?tkn=...&folder=...`).
    pub url: String,
    /// Connection token for openvscode-server authentication.
    pub connection_token: String,
    /// VS Code port on the host.
    pub vscode_port: u16,
    /// `(container_port, host_port)` pairs for user-configured application ports.
    pub port_mappings: Vec<(u16, u16)>,
    /// Pre-allocated spare host ports for dynamic forwarding (socat-based).
    #[serde(default)]
    pub spare_ports: Vec<u16>,
    /// Worktree folder path inside the container — same value embedded in
    /// `url`'s `&folder=` query parameter. Surfaced as a structured field so
    /// callers (notably the GH-45 shared-port proxy URL builder in
    /// `routes::workflows::open_editor`) don't have to re-parse `url`.
    #[serde(default)]
    pub folder: String,
    /// GH-45: CSPRNG path token for the shared-port reverse proxy. Stored as
    /// a container label (`maestro.path_token`) so `get_editor_info` can
    /// return the same token on reconnect, keeping `--server-base-path` in
    /// sync with the proxy registry.
    pub path_token: String,
}

/// Return the deterministic editor container name for a ticket.
fn editor_container_name(ticket_key: &str) -> String {
    format!("maestro-editor-{}", sanitize_ticket_key(ticket_key))
}

/// List host ports already claimed by any Maestro-managed container
/// (`maestro-editor-*` and `maestro-run-*`). Both container families
/// allocate from the same editor port range (9100–9200), so the
/// allocator must see ports from both to avoid collisions.
async fn used_editor_ports() -> Vec<u16> {
    let mut ports = Vec::new();
    // Check both editor and run-command containers that share the port range.
    // In --network=host mode (DinD), Docker's {{.Ports}} field is empty, so we
    // also read the maestro.vscode_port and maestro.spare_ports labels which are
    // always set on editor/run-command containers.
    for filter in ["name=maestro-editor-", "name=maestro-run-"] {
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
                "{{.Label \"maestro.vscode_port\"}} {{.Label \"maestro.spare_ports\"}}",
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
/// Retries with exponential backoff if not enough ports are available,
/// since Docker port bindings may not be immediately visible.
async fn allocate_editor_ports(count: usize) -> Option<Vec<u16>> {
    for attempt in 0..5 {
        let used = used_editor_ports().await;
        let mut free = Vec::new();
        for p in EDITOR_PORT_MIN..=EDITOR_PORT_MAX {
            if !used.contains(&p) {
                free.push(p);
                if free.len() == count {
                    return Some(free);
                }
            }
        }

        // Not enough free ports on this attempt. If this is not the last attempt,
        // wait a bit for Docker to register port bindings and retry.
        if attempt < 4 {
            let delay_ms = 100 * (attempt + 1) as u64; // 100ms, 200ms, 300ms, 400ms
            tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
            debug!(
                attempt,
                needed = count,
                available = free.len(),
                "Retrying port allocation after delay"
            );
        }
    }
    None // not enough free ports after retries
}

/// Allocate a single free port from the editor/terminal port range.
/// Public so the web layer can allocate terminal ports independently.
pub async fn allocate_single_port() -> Option<u16> {
    allocate_editor_ports(1).await.map(|v| v[0])
}

/// Generate a cryptographically random connection token for editor sessions.
/// Returns a 32-character lowercase hex string (UUID v4 simple format).
///
/// NOTE: This token is consumed by `openvscode-server`'s built-in `?tkn=`
/// authentication and `ttyd`'s `-b /TOKEN` base-path. It is NOT the
/// session path token used by the shared-port reverse proxy — see
/// [`generate_session_path_token`] for that.
pub fn generate_connection_token() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// Generate a session path token for the shared-port reverse proxy.
///
/// Returns a 32-character lowercase hex string encoding 16 bytes (128 bits)
/// drawn from the operating system's CSPRNG via [`getrandom`]. UUID v4 is
/// deliberately NOT used here: a v4 UUID has only 122 random bits because
/// six bits encode the version + variant nibbles, which falls below the
/// ≥128-bit entropy floor required by GH-45 for the session URL path.
///
/// Panicking only on `getrandom` failure is acceptable here because:
/// - failure means the kernel CSPRNG is unavailable, in which case we have
///   no business minting URL secrets at all;
/// - the call site in [`crate::container`] runs on a worker thread, not the
///   axum request path, so a panic surfaces as a 500 to the caller, not a
///   silent token reuse.
pub fn generate_session_path_token() -> String {
    let mut buf = [0u8; 16];
    getrandom::fill(&mut buf).expect("OS CSPRNG (getrandom) must be available");
    let mut out = String::with_capacity(32);
    for byte in buf {
        // hex::encode adds a transitive dep we don't need in core. Hand-roll
        // the 32-char lowercase hex encoding to keep the surface small.
        out.push(HEX_DIGITS[(byte >> 4) as usize] as char);
        out.push(HEX_DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

/// Build the editor URL including the connection token for authentication.
pub fn build_editor_url(host_port: u16, connection_token: &str, folder: &str) -> String {
    format!("http://localhost:{host_port}/?tkn={connection_token}&folder={folder}")
}

/// Build the editor URL exposed through the shared-port reverse proxy.
///
/// Returns a relative URL of the shape `/s/<path-token>/?tkn=<conn>&folder=<folder>`
/// so the browser can resolve it against the dashboard origin. The
/// reverse-proxy strips the `/s/<path-token>` prefix and forwards the
/// remainder (including the preserved query string) to the loopback
/// `openvscode-server` listener.
///
/// The `folder` parameter is percent-encoded so paths containing
/// query-string-unsafe characters (`&`, `#`, `=`, `+`, spaces) don't
/// break URL parsing.
pub fn build_session_editor_url(path_token: &str, connection_token: &str, folder: &str) -> String {
    let encoded_folder = encode_query_value(folder);
    format!("/s/{path_token}/?tkn={connection_token}&folder={encoded_folder}")
}

/// Percent-encode a value for safe embedding in a URL query parameter.
///
/// Encodes characters that would otherwise break query-string parsing:
/// `&`, `#`, `=`, `+`, ` `, `?`. Unreserved characters and `/` (common in
/// folder paths) are left as-is to keep URLs readable.
fn encode_query_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'&' | b'#' | b'=' | b'+' | b' ' | b'?' | b'%' => {
                out.push('%');
                out.push(HEX_DIGITS[(b >> 4) as usize] as char);
                out.push(HEX_DIGITS[(b & 0x0f) as usize] as char);
            }
            _ => out.push(b as char),
        }
    }
    out
}

/// Build the terminal URL including the secret base path for authentication.
/// The token is used as a secret URL path segment — only requests to this path
/// are served by ttyd, providing access control equivalent to the editor `?tkn=` pattern.
pub fn build_terminal_url(host_port: u16, token: &str) -> String {
    format!("http://localhost:{host_port}/{token}/")
}

/// Build the terminal URL exposed through the shared-port reverse proxy.
///
/// Returns a relative URL of the shape `/s/<path-token>/<ttyd-token>/`. The
/// outer `<path-token>` is consumed by the proxy registry; the inner
/// `<ttyd-token>` is the existing ttyd `-b /TOKEN` base-path that ttyd itself
/// validates. Both must match for a request to reach the terminal — the
/// proxy is the unguessability layer, ttyd is the in-process defence in depth.
pub fn build_session_terminal_url(path_token: &str, ttyd_token: &str) -> String {
    format!("/s/{path_token}/{ttyd_token}/")
}

/// Build a Docker `--publish` argument string for editor/terminal session ports.
///
/// When running with a local Docker daemon (no `DOCKER_HOST`), binds to the
/// loopback interface only (`127.0.0.1:HOST:CONTAINER`) so the port is
/// reachable only by the maestro-web reverse proxy, not by anyone on the LAN.
///
/// When running with DinD (`DOCKER_HOST` is set), publishes to all interfaces
/// (`HOST:CONTAINER`) so the proxy in the maestro container can reach the
/// port over the Docker Compose network. Direct host access is prevented by
/// removing the DinD port range from docker-compose.dind.yml — the ports are
/// only accessible within the Compose network. See GH-45 acceptance criterion #10.
pub fn session_publish_arg(host_port: u16, container_port: u16) -> String {
    if std::env::var("DOCKER_HOST").is_ok() {
        format!("{host_port}:{container_port}")
    } else {
        format!("127.0.0.1:{host_port}:{container_port}")
    }
}

/// Parse the `maestro.connection_token` value from a Docker inspect JSON labels string.
/// Returns `None` if the label is absent, empty, or the JSON is malformed.
pub fn parse_connection_token_from_labels(json_str: &str) -> Option<String> {
    parse_label_value(json_str, "maestro.connection_token")
}

/// Extract a single label value from a `docker inspect` JSON labels map.
///
/// Returns `None` if the JSON is unparseable, the key is absent, or the
/// value is empty.
pub fn parse_label_value(json_str: &str, key: &str) -> Option<String> {
    let labels: std::collections::HashMap<String, String> = serde_json::from_str(json_str).ok()?;
    let val = labels.get(key)?;
    if val.is_empty() {
        None
    } else {
        Some(val.clone())
    }
}

/// Start a browser VS Code editor container for a workflow.
///
/// Returns [`EditorInfo`] with the URL and port mappings on success.
#[allow(clippy::too_many_arguments)]
pub async fn start_editor(
    ticket_key: &str,
    worktree_path: &Path,
    image: &str,
    _app_ports: &[u16],
    _dynamic_ports: usize,
    theme: &str,
    extensions: &[String],
    settings: &std::collections::HashMap<String, toml::Value>,
    setup_commands: &[String],
    startup_commands: &[String],
    git_editor: &str,
    isolate_workspace: bool,
) -> std::result::Result<EditorInfo, String> {
    let name = editor_container_name(ticket_key);

    // Check if already running
    if let Some(info) = get_editor_info(ticket_key).await {
        return Ok(info);
    }

    // Remove any leftover stopped container with the same name so `docker run` doesn't
    // fail with a name conflict (e.g., after a close-editor that raced with --rm).
    let _ = tokio::process::Command::new("docker")
        .args(["rm", "-f", &name])
        .output()
        .await;

    // Allocate 1 port for the VS Code server. Terminals and dynamic port
    // forwards each allocate their own single port on demand from the same range.
    let ports = allocate_editor_ports(1).await.ok_or_else(|| {
        format!("No free editor ports available (range {EDITOR_PORT_MIN}–{EDITOR_PORT_MAX})")
    })?;

    let vscode_port = ports[0];
    let port_mappings: Vec<(u16, u16)> = Vec::new();
    let spare_ports: Vec<u16> = Vec::new();
    let connection_token = generate_connection_token();
    let path_token = generate_session_path_token();
    info!(ticket = %name, vscode_port, "Allocated editor port");

    let mut args: Vec<String> = vec![
        "run".into(),
        "-d".into(),
        "--name".into(),
        name.clone(),
        "--cap-add=NET_ADMIN".into(),
    ];

    // Environment
    for (k, v) in WORKER_ENV {
        args.push("-e".into());
        args.push(format!("{k}={v}"));
    }
    for key in PASSTHROUGH_ENV {
        if let Ok(val) = std::env::var(key)
            && !val.is_empty()
        {
            args.push("-e".into());
            args.push(format!("{key}={val}"));
        }
    }

    // Volumes — use per-issue isolation when enabled
    for mount in build_volume_args(worktree_path, isolate_workspace) {
        args.push("-v".into());
        args.push(mount);
    }

    // In DinD mode, use `--network=host` so the editor process binds
    // directly to the DinD container's network namespace (shared with
    // maestro via `network_mode: service:dind`).  This bypasses
    // docker-proxy, which does not forward cross-container traffic on
    // Docker Desktop for Mac.
    //
    // In local-Docker mode, publish each port on the host loopback only —
    // external traffic must flow through the shared-port reverse proxy at
    // `/s/<path-token>/`.  See GH-45 acceptance criterion #10.
    if is_dind_mode() {
        args.push("--network=host".into());
    } else {
        args.push("-p".into());
        args.push(session_publish_arg(vscode_port, vscode_port));
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

    // Entrypoint override
    args.push("--entrypoint".into());
    args.push("".into());

    // Labels for get_editor_info() retrieval — essential for `--network=host`
    // containers where `docker port` returns nothing.
    args.push("--label".into());
    args.push(format!("maestro.connection_token={connection_token}"));
    args.push("--label".into());
    args.push(format!("maestro.path_token={path_token}"));
    args.push("--label".into());
    args.push(format!("maestro.vscode_port={vscode_port}"));
    if !spare_ports.is_empty() {
        args.push("--label".into());
        let sp_csv: String = spare_ports.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(",");
        args.push(format!("maestro.spare_ports={sp_csv}"));
    }

    // Image
    args.push(image.into());

    // Build settings.json content from theme + settings map
    let folder = worktree_path.to_string_lossy();
    let mut settings_json = serde_json::Map::new();
    if !theme.is_empty() {
        settings_json.insert(
            "workbench.colorTheme".into(),
            serde_json::Value::String(theme.to_string()),
        );
    }
    // Set the browser tab title to the workflow name (with optional active editor info).
    settings_json.insert(
        "window.title".into(),
        serde_json::Value::String(format!(
            "{ticket_key} — ${{activeEditorShort}}${{separator}}${{rootName}}"
        )),
    );
    for (key, val) in settings {
        settings_json.insert(key.clone(), toml_value_to_json(val));
    }

    // Build shell script: source env, write settings, install extensions, launch
    let mut script_parts: Vec<String> = Vec::new();
    script_parts.push("[ -f /etc/maestro/env ] && set -a && . /etc/maestro/env && set +a".into());

    if !settings_json.is_empty() {
        let json_str = serde_json::to_string(&settings_json).unwrap_or_default();
        let escaped = json_str.replace('\'', "'\\''");
        script_parts.push(format!(
            "mkdir -p ~/.openvscode-server/data/Machine && echo '{}' > ~/.openvscode-server/data/Machine/settings.json",
            escaped
        ));
    }

    for ext in extensions {
        let escaped = shell_escape(ext);
        script_parts.push(format!(
            "openvscode-server --install-extension {escaped} --force 2>/dev/null || true"
        ));
    }

    script_parts.push(format!(
        "exec openvscode-server --port {vscode_port} --host 0.0.0.0 --connection-token {connection_token} --server-base-path /s/{path_token}"
    ));

    let cmd = script_parts.join("; ");
    args.push("sh".into());
    args.push("-c".into());
    args.push(cmd);

    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    info!(
        name = %name,
        vscode_port = vscode_port,
        app_ports = ?port_mappings,
        "Starting editor container"
    );

    let output = tokio::process::Command::new("docker")
        .args(&arg_refs)
        .output()
        .await
        .map_err(|e| format!("Failed to start editor container: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("docker run failed: {stderr}"));
    }

    // Run one-time setup (apt installs, git editor) — gated by marker file.
    if !setup_commands.is_empty() || !git_editor.is_empty() {
        run_editor_setup_as_root(&name, setup_commands, git_editor).await;
    }
    // Run startup commands every time a fresh container is created (no marker file).
    run_editor_startup_commands(&name, startup_commands).await;

    let host_vscode_port = editor_host_port(vscode_port);
    let url = build_editor_url(host_vscode_port, &connection_token, &folder);
    let log_url = format!("http://localhost:{host_vscode_port}/?tkn=<redacted>&folder={folder}");
    info!(url = %log_url, spare = ?spare_ports, "Editor container started");

    Ok(EditorInfo {
        url,
        connection_token,
        vscode_port,
        port_mappings,
        spare_ports,
        folder: folder.into_owned(),
        path_token,
    })
}

/// Run setup commands inside the editor container as root. Ensures ownership of the
/// maestro user's home and cache directories is correct so `mise install` etc. can write.
/// Idempotent via `/tmp/.maestro-terminal-setup-done` marker.
async fn run_editor_setup_as_root(container: &str, setup_commands: &[String], git_editor: &str) {
    // Skip if already done for this container.
    let marker = "/tmp/.maestro-terminal-setup-done";
    let marker_check = tokio::process::Command::new("docker")
        .args(["exec", container, "test", "-f", marker])
        .output()
        .await;
    if matches!(&marker_check, Ok(o) if o.status.success()) {
        return;
    }

    info!(container, "Running editor setup commands as root");

    // Ensure maestro owns its home dir & mise volumes (fresh volumes mount root-owned).
    // Runs as root unconditionally; fast and safe.
    let chown_script = r#"
chown -R maestro:maestro /home/maestro/.local/share/mise /home/maestro/.cache/mise 2>/dev/null || true
chown -R maestro:maestro /home/maestro/.config/mise 2>/dev/null || true
"#;
    let _ = tokio::process::Command::new("docker")
        .args([
            "exec",
            "--user",
            "root",
            container,
            "bash",
            "-lc",
            chown_script,
        ])
        .output()
        .await;

    // Install and configure the git editor if specified.
    if !git_editor.is_empty() {
        let escaped = shell_escape(git_editor);
        let install_script = format!(
            "apt-get install -y --no-install-recommends {escaped} 2>&1 \
             && su - maestro -c 'git config --global core.editor {escaped}'"
        );
        let out = tokio::process::Command::new("docker")
            .args([
                "exec",
                "--user",
                "root",
                container,
                "bash",
                "-lc",
                &install_script,
            ])
            .output()
            .await;
        match out {
            Ok(o) if o.status.success() => {
                info!(container, git_editor, "Git editor installed and configured");
            }
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                warn!(container, git_editor, %stderr, "Git editor install failed");
            }
            Err(e) => {
                warn!(container, git_editor, error = %e, "Git editor install error");
            }
        }
    }

    // Configure gh as the git credential helper so `git push` works without prompting.
    // Equivalent to what entrypoint.sh does for the main container.
    let _ = tokio::process::Command::new("docker")
        .args([
            "exec",
            "--user",
            "root",
            container,
            "bash",
            "-lc",
            "su - maestro -c 'gh auth setup-git 2>/dev/null || true'",
        ])
        .output()
        .await;

    // Run user-defined setup_commands as the maestro user.
    let out = if !setup_commands.is_empty() {
        let joined = setup_commands.join(" && echo && ");
        let wrapped = format!(
            "[ -f /etc/maestro/env ] && set -a && . /etc/maestro/env && set +a; {joined} && mise reshim 2>&1 || true"
        );
        tokio::process::Command::new("docker")
            .args([
                "exec",
                "--user",
                "root",
                container,
                "bash",
                "-lc",
                &format!(r#"su - maestro -c {}"#, shell_escape(&wrapped)),
            ])
            .output()
            .await
            .map(Some)
    } else {
        Ok(None)
    };
    let out = match out {
        Ok(Some(o)) => Some(o),
        Ok(None) => None,
        Err(e) => {
            warn!(container, error = %e, "Failed to run editor setup commands");
            None
        }
    };

    match out {
        Some(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            info!(container, %stdout, "Editor setup commands completed");
            // Mark done so subsequent calls skip.
            let _ = tokio::process::Command::new("docker")
                .args(["exec", container, "touch", marker])
                .output()
                .await;
        }
        Some(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let stderr = String::from_utf8_lossy(&o.stderr);
            warn!(
                container,
                code = ?o.status.code(),
                %stdout,
                %stderr,
                "Editor setup commands failed (continuing without marker — will retry)"
            );
        }
        None => {
            // No user setup_commands; touch the marker so git editor install isn't re-run.
            let _ = tokio::process::Command::new("docker")
                .args(["exec", container, "touch", marker])
                .output()
                .await;
        }
    }
}

/// Run `startup_commands` as the maestro user inside the editor container.
///
/// Unlike `run_editor_setup_as_root` this has **no marker file** — it runs every time
/// a fresh container is created. Use for idempotent commands like `mise use -g ruby@3.3`
/// that should verify/update tool versions on each editor open.
async fn run_editor_startup_commands(container: &str, cmds: &[String]) {
    if cmds.is_empty() {
        return;
    }
    info!(container, "Running editor startup commands");
    let joined = cmds.join(" && echo && ");
    let wrapped = format!(
        "[ -f /etc/maestro/env ] && set -a && . /etc/maestro/env && set +a; {joined} && mise reshim 2>&1 || true"
    );
    let out = tokio::process::Command::new("docker")
        .args([
            "exec",
            "--user",
            "root",
            container,
            "bash",
            "-lc",
            &format!("su - maestro -c {}", shell_escape(&wrapped)),
        ])
        .output()
        .await;
    match out {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            info!(container, %stdout, "Editor startup commands completed");
        }
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            let stderr = String::from_utf8_lossy(&o.stderr);
            warn!(
                container,
                code = ?o.status.code(),
                %stdout,
                %stderr,
                "Editor startup commands failed (continuing)"
            );
        }
        Err(e) => {
            warn!(container, error = %e, "Failed to run editor startup commands");
        }
    }
}

/// Stop and remove an editor container for a workflow.
pub async fn stop_editor(ticket_key: &str) {
    let name = editor_container_name(ticket_key);
    let _ = tokio::process::Command::new("docker")
        .args(["rm", "-f", &name])
        .output()
        .await;
    info!(name = %name, "Editor container stopped");
}

/// Start a web-based terminal (ttyd) inside the running editor container on `port`.
/// Returns the URL on success. Setup commands (tool installs, etc.) are expected to
/// have already been run at editor container creation by `run_editor_setup_as_root`.
pub async fn start_terminal(
    ticket_key: &str,
    port: u16,
) -> std::result::Result<(String, String), String> {
    let name = editor_container_name(ticket_key);

    // Check the editor container is actually running.
    if get_editor_info(ticket_key).await.is_none() {
        return Err("Editor container is not running — open the editor first.".into());
    }

    // Build the shell script that runs in each ttyd terminal:
    // 1. Source the maestro env file (Claude auth tokens, API keys, etc.)
    // 2. Auto-restore the most recent ~/.claude.json backup if missing (Claude Code
    //    looks for this file — restoring avoids the first-run wizard each session).
    // 3. Exec a login shell so /etc/profile.d/*.sh (mise shims, etc.) are loaded.
    // NOTE: Tool installs (setup_commands) run at editor CONTAINER CREATION as root
    //       via `run_editor_setup_as_root`, not here.
    // `~/.claude.json` lives in the home dir (NOT inside `~/.claude/`), so it is NOT
    // covered by the /shared-auth/claude volume. We symlink it into ~/.claude/ so
    // auth state persists across container restarts.
    // `~/.claude.json` lives in the home dir (NOT inside `~/.claude/`), so it is NOT
    // covered by the /shared-auth/claude volume. We symlink it into ~/.claude/ so
    // auth state persists across container restarts.
    //
    // Claude Code in INTERACTIVE mode triggers a login wizard unless `.claude.json`
    // contains `hasCompletedOnboarding: true` — even when CLAUDE_CODE_OAUTH_TOKEN
    // and ANTHROPIC_BASE_URL are set. We inject that field on startup so Claude
    // uses the env-var auth (same as the headless --print mode used by workflows).
    let shell_cmd = r#"[ -f /etc/maestro/env ] && set -a && . /etc/maestro/env && set +a
# Source the centralized GitHub App token so gh CLI and git operations authenticate.
if [ -f "$HOME/.config/gh/gh-app-token" ]; then
  export GH_TOKEN="$(cat "$HOME/.config/gh/gh-app-token")"
  # Configure git credential helper to use the token file (editor containers
  # don't inherit the main container's ~/.gitconfig).
  git config --global credential.https://github.com.helper \
    '!f() { echo protocol=https; echo host=github.com; echo username=x-access-token; echo "password=$(cat $HOME/.config/gh/gh-app-token 2>/dev/null || echo $GH_TOKEN)"; }; f' 2>/dev/null
fi
# Make ~/.claude.json persistent by symlinking into the shared volume.
if [ ! -L "$HOME/.claude.json" ]; then
  if [ -f "$HOME/.claude.json" ] && [ ! -f "$HOME/.claude/.claude.json" ]; then
    mv "$HOME/.claude.json" "$HOME/.claude/.claude.json"
  elif [ ! -f "$HOME/.claude/.claude.json" ] && ls "$HOME/.claude/backups/.claude.json.backup."* >/dev/null 2>&1; then
    latest=$(ls -t "$HOME/.claude/backups/.claude.json.backup."* | head -1)
    cp "$latest" "$HOME/.claude/.claude.json"
  fi
  rm -f "$HOME/.claude.json"
  ln -s "$HOME/.claude/.claude.json" "$HOME/.claude.json"
fi
# Ensure hasCompletedOnboarding=true to skip the interactive login wizard.
# If the existing file already has the field set to true, leave it alone (preserves
# other state). Otherwise, write a minimal config — Claude uses env vars for auth.
if ! grep -qE '"hasCompletedOnboarding"[[:space:]]*:[[:space:]]*true' "$HOME/.claude/.claude.json" 2>/dev/null; then
  echo '{"hasCompletedOnboarding":true}' > "$HOME/.claude/.claude.json"
fi
exec bash -l"#.to_string();
    let token = generate_connection_token();
    let base_path = format!("/{token}");
    let tab_title = format!("titleFixed={ticket_key} — Terminal");
    info!(ticket = %ticket_key, port, "Starting ttyd on port");
    let output = tokio::process::Command::new("docker")
        .args([
            "exec",
            "-d",
            &name,
            "ttyd",
            "-p",
            &port.to_string(),
            "-W",
            "-b",
            &base_path,
            "-t",
            "fontSize=14",
            "-t",
            &tab_title,
            "bash",
            "-c",
            &shell_cmd,
        ])
        .output()
        .await
        .map_err(|e| format!("Failed to start ttyd: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("ttyd start failed: {stderr}"));
    }

    // Verify ttyd is actually listening on the port with a few retries.
    // Use bash's /dev/tcp pseudo-device inside the container: no nc/socat needed,
    // and it runs inside the editor container's own network namespace so it works
    // regardless of DinD network topology.
    for attempt in 0..5 {
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
        let nc_check = tokio::process::Command::new("docker")
            .args([
                "exec",
                &name,
                "bash",
                "-c",
                &format!("echo > /dev/tcp/127.0.0.1/{port}"),
            ])
            .output()
            .await;
        if matches!(nc_check, Ok(ref o) if o.status.success()) {
            let host_port = editor_host_port(port);
            let url = build_terminal_url(host_port, &token);
            info!(ticket = %ticket_key, container_port = port, host_port, "Web terminal verified listening (token redacted)");
            return Ok((url, token));
        }
        if attempt < 4 {
            debug!(ticket = %ticket_key, port, attempt = attempt + 1, "ttyd not yet listening, retrying");
        }
    }

    Err(format!(
        "ttyd failed to bind to port {port} — verify no other process is using this port"
    ))
}

/// Return the container port that ttyd is currently listening on inside the editor container,
/// or `None` if ttyd is not running.  Uses `pgrep -a ttyd` to read the actual `-p PORT` argument
/// so the result is always correct regardless of what was recorded in memory.
pub async fn find_running_terminal(ticket_key: &str) -> Option<(u16, String)> {
    let name = editor_container_name(ticket_key);
    let out = tokio::process::Command::new("docker")
        .args(["exec", &name, "pgrep", "-a", "ttyd"])
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    parse_terminal_auth_from_pgrep(&stdout)
}

/// Parse both the `-p PORT` and `-b /TOKEN` values from `pgrep -a ttyd` output.
/// Returns `None` if either value is missing or the port is invalid.
/// The leading `/` is stripped from the base-path value.
pub fn parse_terminal_auth_from_pgrep(pgrep_output: &str) -> Option<(u16, String)> {
    pgrep_output
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            let port = parts
                .windows(2)
                .find(|w| w[0] == "-p")
                .and_then(|w| w[1].parse::<u16>().ok())?;
            let base = parts.windows(2).find(|w| w[0] == "-b").map(|w| w[1])?;
            let token = base.strip_prefix('/')?;
            if token.is_empty() {
                return None;
            }
            Some((port, token.to_string()))
        })
        .next()
}

/// Kill the ttyd process inside the editor container.
pub async fn stop_terminal(ticket_key: &str) {
    let name = editor_container_name(ticket_key);
    let _ = tokio::process::Command::new("docker")
        .args(["exec", &name, "pkill", "-x", "ttyd"])
        .output()
        .await;
}

/// Check if an editor container is running and return its info.
pub async fn get_editor_info(ticket_key: &str) -> Option<EditorInfo> {
    let name = editor_container_name(ticket_key);

    let label_output = tokio::process::Command::new("docker")
        .args([
            "inspect",
            &name,
            "--format",
            "{{.State.Running}} {{json .Config.Labels}}",
        ])
        .output()
        .await
        .ok()?;

    if !label_output.status.success() {
        return None;
    }

    let label_stdout = String::from_utf8_lossy(&label_output.stdout);
    let label_stdout = label_stdout.trim();

    // Parse "true {\"maestro.connection_token\":\"...\", ...}" format
    // First word is running state, rest is JSON labels.
    let (running_str, labels_json) = label_stdout.split_once(' ')?;
    if running_str != "true" {
        return None; // Container not running.
    }

    // Extract connection token from labels. Containers without the token
    // (pre-existing from before this security feature) are treated as absent
    // so the user must close and reopen the editor to get a secure session.
    let connection_token = parse_connection_token_from_labels(labels_json)?;

    // GH-45: path token is required for the shared-port proxy. Containers
    // without it must be closed and reopened to get a proxied session.
    let path_token = parse_label_value(labels_json, "maestro.path_token")
        .unwrap_or_default();

    // Port discovery: prefer labels (works for both `--network=host` and
    // `-p` containers) with `docker port` as fallback for pre-label containers.
    let (vscode_port, spare_ports, port_mappings) =
        if let Some(vp_str) = parse_label_value(labels_json, "maestro.vscode_port") {
            let vp: u16 = vp_str.parse().ok()?;
            let sp: Vec<u16> = parse_label_value(labels_json, "maestro.spare_ports")
                .unwrap_or_default()
                .split(',')
                .filter(|s| !s.is_empty())
                .filter_map(|s| s.parse().ok())
                .collect();
            (vp, sp, Vec::new())
        } else {
            // Fallback: parse `docker port` output (pre-label containers).
            let port_output = tokio::process::Command::new("docker")
                .args(["port", &name])
                .output()
                .await
                .ok()?;
            if !port_output.status.success() {
                return None;
            }
            let stdout = String::from_utf8_lossy(&port_output.stdout);
            let mut pm = Vec::new();
            let mut erp: Vec<u16> = Vec::new();
            for line in stdout.lines() {
                let parts: Vec<&str> = line.split("->").collect();
                if parts.len() != 2 { continue; }
                let cp: u16 = parts[0].trim().split('/').next()
                    .and_then(|s| s.parse().ok()).unwrap_or(0);
                let hp: u16 = parts[1].trim().rsplit(':').next()
                    .and_then(|s| s.parse().ok()).unwrap_or(0);
                if cp == 0 || hp == 0 { continue; }
                if (EDITOR_PORT_MIN..=EDITOR_PORT_MAX).contains(&cp) && cp == hp {
                    if !erp.contains(&hp) { erp.push(hp); }
                } else if !(EDITOR_PORT_MIN..=EDITOR_PORT_MAX).contains(&cp)
                    && !pm.contains(&(cp, hp))
                {
                    pm.push((cp, hp));
                }
            }
            erp.sort();
            let vp = *erp.first()?;
            let sp: Vec<u16> = erp.into_iter().skip(1).collect();
            (vp, sp, pm)
        };

    // Get the working directory to reconstruct the folder URL
    let wd_output = tokio::process::Command::new("docker")
        .args(["inspect", &name, "--format", "{{.Config.WorkingDir}}"])
        .output()
        .await
        .ok()?;

    let folder = String::from_utf8_lossy(&wd_output.stdout)
        .trim()
        .to_string();
    let host_vscode_port = editor_host_port(vscode_port);
    let url = build_editor_url(host_vscode_port, &connection_token, &folder);

    Some(EditorInfo {
        url,
        connection_token,
        vscode_port,
        port_mappings,
        spare_ports,
        folder,
        path_token,
    })
}

/// List and force-remove containers whose name matches the prefix.
async fn remove_containers_matching(sanitized_key: &str) {
    let filter = format!("name=maestro-worker-{sanitized_key}-");
    let output = tokio::process::Command::new("docker")
        .args(["ps", "-a", "--filter", &filter, "-q"])
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let ids: Vec<&str> = stdout.lines().filter(|l| !l.is_empty()).collect();
            if ids.is_empty() {
                return;
            }
            info!(
                count = ids.len(),
                key = sanitized_key,
                "Removing worker containers"
            );
            let mut rm_args: Vec<&str> = vec!["rm", "-f"];
            rm_args.extend(ids.iter());
            let _ = tokio::process::Command::new("docker")
                .args(&rm_args)
                .output()
                .await;
        }
        Ok(out) => {
            warn!(
                stderr = %String::from_utf8_lossy(&out.stderr),
                "docker ps failed while cleaning up worker containers"
            );
        }
        Err(e) => {
            warn!(error = %e, "Failed to list worker containers for cleanup");
        }
    }
}

/// Prune dangling DinD images (throttled to once per 5 minutes).
///
/// Runs `docker image prune -f` to remove dangling image layers that accumulate
/// from rebuilding `maestro:latest`. This is safe because dangling images have no
/// tags and are not referenced by any running container. The `maestro:latest`
/// image itself is always tagged and will never be removed.
async fn prune_dangling_images() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let last = LAST_IMAGE_PRUNE.load(Ordering::Relaxed);
    if now.saturating_sub(last) < IMAGE_PRUNE_INTERVAL_SECS {
        return; // throttled
    }
    LAST_IMAGE_PRUNE.store(now, Ordering::Relaxed);

    let output = tokio::process::Command::new("docker")
        .args(["image", "prune", "-f"])
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            if !stdout.trim().is_empty() {
                info!("Pruned dangling DinD images: {}", stdout.trim());
            }
        }
        Ok(out) => warn!(
            stderr = %String::from_utf8_lossy(&out.stderr),
            "docker image prune failed"
        ),
        Err(e) => warn!(error = %e, "Failed to run docker image prune"),
    }
}

// ---------------------------------------------------------------------------
// Dynamic port forwarding — background port scanner
// ---------------------------------------------------------------------------

/// Scan listening ports inside an editor container and socat-forward them to spare
/// host ports. Works for apps binding to either 0.0.0.0 or 127.0.0.1 inside the
/// container, because socat runs inside the container and connects via `localhost`.
///
/// `spare_ports` is the pool of symmetric-mapped host ports available for forwarding.
pub async fn run_port_scanner(
    ticket_key: &str,
    vscode_port: u16,
    spare_ports: Vec<u16>,
    event_tx: broadcast::Sender<WorkflowEvent>,
    cancel: CancellationToken,
) {
    let container = editor_container_name(ticket_key);
    let ticket = ticket_key.to_string();

    // Ports to never treat as "new": VS Code and all pre-allocated spare ports (they
    // are docker-mapped; socat may briefly keep them LISTENing after kill, so never
    // re-forward them).
    let mut always_ignore: std::collections::HashSet<u16> = std::collections::HashSet::new();
    always_ignore.insert(vscode_port);
    for sp in &spare_ports {
        always_ignore.insert(*sp);
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

        let listening = match scan_listening_ports(&container).await {
            Some(ports) => ports,
            None => continue,
        };
        let listening_set: std::collections::HashSet<u16> =
            listening.iter().map(|(p, _)| *p).collect();

        // Detect new listening ports → start socat forwarding.
        for &(port, family) in &listening {
            if always_ignore.contains(&port) || active_forwards.contains_key(&port) {
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
                });
            }
        }
    }
}

/// Address family detected for a listener. Affects socat's connect side so we
/// reach apps regardless of whether they bind to 127.0.0.1 or ::1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ListenFamily {
    Ipv4,
    Ipv6,
}

/// Run `ss -tlnH` inside the container and return listening `(port, family)` entries.
/// If the same port is listening on both families, prefer IPv4 (better Docker reachability).
async fn scan_listening_ports(container: &str) -> Option<Vec<(u16, ListenFamily)>> {
    let output = tokio::process::Command::new("docker")
        .args(["exec", container, "ss", "-tlnH"])
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut by_port: HashMap<u16, ListenFamily> = HashMap::new();
    // Format: "LISTEN 0 128 0.0.0.0:6006  0.0.0.0:*"
    //    or:  "LISTEN 0 128 [::]:6006     [::]:*"
    //    or:  "LISTEN 0 128 127.0.0.1:5173  0.0.0.0:*"
    //    or:  "LISTEN 0 128 [::1]:5173     [::]:*"
    for line in stdout.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() >= 4 {
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
    }
    Some(by_port.into_iter().collect())
}

/// Return the set of ports currently listening inside the editor container.
/// Used by `open_terminal` to avoid picking a spare port already bound by socat.
/// Returns an empty set if the container is unreachable or `ss` fails.
pub async fn listening_ports_in_editor(ticket_key: &str) -> std::collections::HashSet<u16> {
    let name = editor_container_name(ticket_key);
    scan_listening_ports(&name)
        .await
        .map(|v| v.into_iter().map(|(p, _)| p).collect())
        .unwrap_or_default()
}

/// Start a `socat` process inside the container to forward `spare_port` → `target_port`.
async fn start_socat(
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
async fn kill_socat(container: &str, spare_port: u16) {
    // Match either TCP-LISTEN or TCP4-LISTEN with the spare port.
    let pattern = format!("LISTEN:{spare_port}");
    let _ = tokio::process::Command::new("docker")
        .args(["exec", container, "pkill", "-f", &pattern])
        .output()
        .await;
}

// ---------------------------------------------------------------------------
// Run commands — dedicated containers for user-defined shell commands
// ---------------------------------------------------------------------------

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
pub async fn start_run_command(
    ticket_key: &str,
    worktree_path: &Path,
    image: &str,
    command: &str,
    cmd_index: usize,
    _dynamic_ports: usize,
    isolate_workspace: bool,
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

    // Allocate 1 port for the run command container.
    let spare_ports = allocate_editor_ports(1).await.ok_or_else(|| {
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
    for key in PASSTHROUGH_ENV {
        if let Ok(val) = std::env::var(key)
            && !val.is_empty()
        {
            args.push("-e".into());
            args.push(format!("{key}={val}"));
        }
    }

    // Volumes — use per-issue isolation when enabled
    for mount in build_volume_args(worktree_path, isolate_workspace) {
        args.push("-v".into());
        args.push(mount);
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
pub async fn run_run_command_port_scanner(
    ticket_key: &str,
    cmd_index: usize,
    spare_ports: Vec<u16>,
    event_tx: broadcast::Sender<WorkflowEvent>,
    cancel: CancellationToken,
) {
    let container = run_command_container_name(ticket_key, cmd_index);
    let ticket = ticket_key.to_string();

    // Ports to never treat as "new": all pre-allocated spare ports
    let mut always_ignore: std::collections::HashSet<u16> = std::collections::HashSet::new();
    for sp in &spare_ports {
        always_ignore.insert(*sp);
    }

    let mut active_forwards: HashMap<u16, u16> = HashMap::new();
    let mut available_spares: Vec<u16> = spare_ports;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                for (&detected, &spare) in &active_forwards {
                    kill_socat(&container, spare).await;
                    debug!(ticket = %ticket, cmd_index, detected, spare, "Cleaned up run-cmd socat on scanner shutdown");
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
            });
            // Clean up the stopped container (we removed --rm to capture exit info).
            let _ = tokio::process::Command::new("docker")
                .args(["rm", "-f", &container])
                .output()
                .await;
            return;
        }

        let listening = match scan_listening_ports(&container).await {
            Some(ports) => ports,
            None => continue,
        };
        let listening_set: std::collections::HashSet<u16> =
            listening.iter().map(|(p, _)| *p).collect();

        // Detect new listening ports → start socat forwarding.
        for &(port, family) in &listening {
            if always_ignore.contains(&port) || active_forwards.contains_key(&port) {
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
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn runner() -> ContainerRunner {
        ContainerRunner::new(
            "PROJ-42",
            &PathBuf::from("/workspace/proj-42"),
            "maestro:latest",
        )
    }

    #[test]
    fn sanitize_ticket_key_lowercases_and_replaces() {
        assert_eq!(sanitize_ticket_key("PROJ-123"), "proj-123");
        assert_eq!(sanitize_ticket_key("My_Ticket.1"), "my-ticket-1");
    }

    #[test]
    fn next_container_name_increments() {
        let r = runner();
        assert_eq!(r.next_container_name(), "maestro-worker-proj-42-0");
        assert_eq!(r.next_container_name(), "maestro-worker-proj-42-1");
        assert_eq!(r.next_container_name(), "maestro-worker-proj-42-2");
    }

    /// Helper: find the value following a flag in a docker args list.
    fn flag_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
        args.windows(2).find_map(|w| {
            if w[0] == flag {
                Some(w[1].as_str())
            } else {
                None
            }
        })
    }

    /// Helper: check if an `-e KEY=VALUE` pair is present.
    fn has_env(args: &[String], key: &str, value: &str) -> bool {
        let needle = format!("{key}={value}");
        args.windows(2).any(|w| w[0] == "-e" && w[1] == needle)
    }

    /// Helper: check if a `-v SRC:DST` pair is present.
    fn has_volume(args: &[String], mount: &str) -> bool {
        args.windows(2).any(|w| w[0] == "-v" && w[1] == mount)
    }

    #[test]
    fn wrap_command_structure() {
        let r = runner();
        let (program, args) = r.wrap_command("claude", &["--print", "-p", "hello"]);

        assert_eq!(program, "docker");
        assert_eq!(args[0], "run");
        assert_eq!(args[1], "--rm");

        // Container name
        assert_eq!(
            flag_value(&args, "--name"),
            Some("maestro-worker-proj-42-0")
        );

        // NET_ADMIN
        assert!(args.contains(&"--cap-add=NET_ADMIN".to_string()));

        // Key env vars
        assert!(has_env(&args, "HOME", "/home/maestro"));
        assert!(!has_env(&args, "DOCKER_HOST", "tcp://dind:2375"));
        assert!(has_env(&args, "MISE_TRUST_ALL_CONFIGS", "1"));

        // Volume mounts
        assert!(has_volume(&args, "/workspace:/workspace"));
        assert!(has_volume(
            &args,
            "/shared-auth/claude:/home/maestro/.claude"
        ));
        assert!(has_volume(
            &args,
            "/shared-auth/gh:/home/maestro/.config/gh"
        ));

        // Working directory
        assert_eq!(flag_value(&args, "-w"), Some("/workspace/proj-42"));

        // Entrypoint is empty (bypass image entrypoint)
        assert_eq!(flag_value(&args, "--entrypoint"), Some(""));

        assert_eq!(flag_value(&args, "--user"), Some("maestro:maestro"));

        // After --entrypoint "" comes: --user maestro:maestro, image, sh, -c, "restore; exec ..."
        let entrypoint_idx = args.iter().position(|a| a == "--entrypoint").unwrap();
        let tail = &args[entrypoint_idx + 2..];
        assert_eq!(tail[0], "--user");
        assert_eq!(tail[1], "maestro:maestro");
        assert_eq!(tail[2], "maestro:latest");
        assert_eq!(tail[3], "sh");
        assert_eq!(tail[4], "-c");
        // The shell command restores .claude.json then execs the original program
        assert!(
            tail[5].contains("exec claude --print -p hello"),
            "sh -c body: {}",
            tail[5]
        );
    }

    #[test]
    fn wrap_shell_command_uses_worker_entrypoint() {
        let r = runner();
        let (program, args) = r.wrap_shell_command("npm install && npm test");

        assert_eq!(program, "docker");

        // Entrypoint is the worker entrypoint
        assert_eq!(
            flag_value(&args, "--entrypoint"),
            Some("/usr/local/bin/worker-entrypoint.sh")
        );

        // Image + shell command at the tail
        let entrypoint_idx = args.iter().position(|a| a == "--entrypoint").unwrap();
        let tail = &args[entrypoint_idx + 2..];
        assert_eq!(tail[0], "maestro:latest");
        assert_eq!(tail[1], "sh");
        assert_eq!(tail[2], "-c");
        assert_eq!(tail[3], "npm install && npm test");
    }

    #[test]
    fn wrap_command_counter_advances_across_calls() {
        let r = runner();
        let (_, args1) = r.wrap_command("echo", &["a"]);
        let (_, args2) = r.wrap_shell_command("echo b");

        assert_eq!(
            flag_value(&args1, "--name"),
            Some("maestro-worker-proj-42-0")
        );
        assert_eq!(
            flag_value(&args2, "--name"),
            Some("maestro-worker-proj-42-1")
        );
    }

    #[test]
    fn all_fixed_env_vars_present() {
        let r = runner();
        let (_, args) = r.wrap_command("true", &[]);

        for (k, v) in WORKER_ENV {
            assert!(has_env(&args, k, v), "Missing env var {k}={v}");
        }
    }

    #[test]
    fn all_volume_mounts_present() {
        let r = runner();
        let (_, args) = r.wrap_command("true", &[]);

        for mount in WORKER_VOLUMES {
            assert!(has_volume(&args, mount), "Missing volume mount {mount}");
        }
    }

    #[test]
    fn generate_connection_token_is_valid_hex() {
        let token = generate_connection_token();
        assert_eq!(token.len(), 32, "Token must be 32 hex characters");
        assert!(
            token.chars().all(|c| c.is_ascii_hexdigit()),
            "Token must be lowercase hex: {token}"
        );
        assert_eq!(token, token.to_lowercase(), "Token must be lowercase");
    }

    #[test]
    fn generate_connection_token_is_unique() {
        let t1 = generate_connection_token();
        let t2 = generate_connection_token();
        assert_ne!(t1, t2, "Two generated tokens must be different");
    }

    // ---------------------------------------------------------------------
    // GH-45: session path token (CSPRNG, ≥128 bits) and loopback publish arg
    // ---------------------------------------------------------------------

    #[test]
    fn session_path_token_is_32_char_lowercase_hex() {
        let token = generate_session_path_token();
        assert_eq!(
            token.len(),
            32,
            "Token must be 32 hex characters (16 bytes)"
        );
        assert!(
            token.chars().all(|c| c.is_ascii_hexdigit()),
            "Token must be hex: {token}"
        );
        assert_eq!(token, token.to_lowercase(), "Token must be lowercase");
    }

    #[test]
    fn session_path_token_is_unique() {
        // Statistical: with 128 bits of entropy, 1024 generations should
        // produce 1024 distinct values with overwhelming probability.
        let mut seen = std::collections::HashSet::new();
        for _ in 0..1024 {
            let t = generate_session_path_token();
            assert!(
                seen.insert(t),
                "duplicate token in 1024 generations — entropy too low?"
            );
        }
    }

    #[test]
    fn session_path_token_is_not_uuid_v4_shape() {
        // UUID v4 simple has fixed bits at positions 12 (always '4') and 16
        // (always one of '8','9','a','b'). A pure 16-byte random token must
        // not impose those constraints. Verify across many samples that we
        // observe values outside the UUID v4 alphabet at those positions.
        let mut pos12 = std::collections::HashSet::new();
        let mut pos16 = std::collections::HashSet::new();
        for _ in 0..512 {
            let t = generate_session_path_token();
            pos12.insert(t.as_bytes()[12]);
            pos16.insert(t.as_bytes()[16]);
        }
        // We expect each set to contain more than 1 distinct hex digit at
        // those positions — UUID v4 would lock pos12 to b'4' and pos16 to
        // {b'8',b'9',b'a',b'b'}.
        assert!(
            pos12.len() > 1,
            "position 12 was constant — looks UUID-shaped"
        );
        assert!(
            !(pos12 == [b'4'].into_iter().collect()),
            "position 12 locked to '4'"
        );
        assert!(
            pos16.len() > 4,
            "position 16 alphabet too narrow — looks UUID-shaped"
        );
    }

    #[test]
    fn build_session_editor_url_uses_relative_proxy_path() {
        let url = build_session_editor_url(
            "0123456789abcdef0123456789abcdef",
            "deadbeefdeadbeefdeadbeefdeadbeef",
            "/workspace/proj",
        );
        assert_eq!(
            url,
            "/s/0123456789abcdef0123456789abcdef/?tkn=deadbeefdeadbeefdeadbeefdeadbeef&folder=/workspace/proj"
        );
    }

    #[test]
    fn build_session_editor_url_encodes_special_chars_in_folder() {
        let url = build_session_editor_url("tok", "conn", "/workspace/my project&foo#bar");
        assert_eq!(url, "/s/tok/?tkn=conn&folder=/workspace/my%20project%26foo%23bar");
    }

    #[test]
    fn encode_query_value_preserves_slashes_and_alphanumerics() {
        assert_eq!(encode_query_value("/workspace/proj-name"), "/workspace/proj-name");
    }

    #[test]
    fn encode_query_value_encodes_query_unsafe_chars() {
        assert_eq!(encode_query_value("a&b=c#d+e f%g"), "a%26b%3dc%23d%2be%20f%25g");
    }

    #[test]
    fn build_session_terminal_url_uses_relative_proxy_path() {
        let url = build_session_terminal_url(
            "0123456789abcdef0123456789abcdef",
            "deadbeefdeadbeefdeadbeefdeadbeef",
        );
        assert_eq!(
            url,
            "/s/0123456789abcdef0123456789abcdef/deadbeefdeadbeefdeadbeefdeadbeef/"
        );
    }

    #[test]
    fn session_publish_arg_format_matches_env() {
        let arg = session_publish_arg(9101, 9101);
        if std::env::var("DOCKER_HOST").is_ok() {
            // DinD mode: no loopback prefix, just host:container.
            assert_eq!(arg, "9101:9101");
            assert_eq!(session_publish_arg(9201, 9101), "9201:9101");
        } else {
            // Local Docker: loopback-only binding.
            assert_eq!(arg, "127.0.0.1:9101:9101");
            assert_eq!(session_publish_arg(9201, 9101), "127.0.0.1:9201:9101");
        }
    }

    #[test]
    fn build_editor_url_includes_tkn_param() {
        let url = build_editor_url(9100, "abcdef0123456789abcdef0123456789", "/workspace/proj");
        assert_eq!(
            url,
            "http://localhost:9100/?tkn=abcdef0123456789abcdef0123456789&folder=/workspace/proj"
        );
    }

    #[test]
    fn parse_connection_token_from_labels_present() {
        let json = r#"{"maestro.connection_token":"abcdef0123456789abcdef0123456789","other":"x"}"#;
        assert_eq!(
            parse_connection_token_from_labels(json),
            Some("abcdef0123456789abcdef0123456789".to_string())
        );
    }

    #[test]
    fn parse_connection_token_from_labels_missing() {
        let json = r#"{"other.label":"x"}"#;
        assert_eq!(parse_connection_token_from_labels(json), None);
    }

    #[test]
    fn parse_connection_token_from_labels_empty_value() {
        let json = r#"{"maestro.connection_token":""}"#;
        assert_eq!(parse_connection_token_from_labels(json), None);
    }

    #[test]
    fn parse_connection_token_from_labels_invalid_json() {
        assert_eq!(parse_connection_token_from_labels("not json"), None);
        assert_eq!(parse_connection_token_from_labels(""), None);
    }

    #[test]
    fn editor_info_serializes_connection_token() {
        let info = EditorInfo {
            url: "http://localhost:9100/?tkn=abc&folder=/w".to_string(),
            connection_token: "abc".to_string(),
            vscode_port: 9100,
            port_mappings: vec![],
            spare_ports: vec![],
            folder: "/w".to_string(),
            path_token: "deadbeef".to_string(),
        };
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["connection_token"], "abc");
        assert_eq!(json["url"], "http://localhost:9100/?tkn=abc&folder=/w");
    }

    // ── Terminal authentication tests ──────────────────────────────────

    #[test]
    fn build_terminal_url_includes_token_in_path() {
        let url = build_terminal_url(9150, "abcdef0123456789abcdef0123456789");
        assert_eq!(
            url,
            "http://localhost:9150/abcdef0123456789abcdef0123456789/"
        );
    }

    #[test]
    fn build_terminal_url_trailing_slash() {
        let url = build_terminal_url(9100, "aabb");
        assert!(url.ends_with('/'), "Terminal URL must end with /: {url}");
        // The token is immediately before the trailing slash.
        assert!(
            url.ends_with("aabb/"),
            "Token must be immediately before trailing slash: {url}"
        );
    }

    #[test]
    fn parse_terminal_auth_from_pgrep_normal() {
        let output =
            "42 ttyd -p 9150 -W -b /abcdef0123456789abcdef0123456789 -t fontSize=14 bash -c ls\n";
        assert_eq!(
            parse_terminal_auth_from_pgrep(output),
            Some((9150, "abcdef0123456789abcdef0123456789".to_string()))
        );
    }

    #[test]
    fn parse_terminal_auth_from_pgrep_reversed_flag_order() {
        let output = "42 ttyd -b /aabb1122 -p 9200 -W bash -c ls\n";
        assert_eq!(
            parse_terminal_auth_from_pgrep(output),
            Some((9200, "aabb1122".to_string()))
        );
    }

    #[test]
    fn parse_terminal_auth_from_pgrep_missing_base_path() {
        // ttyd running without -b flag → None (treated as unauthenticated / absent)
        let output = "42 ttyd -p 9150 -W -t fontSize=14 bash -c ls\n";
        assert_eq!(parse_terminal_auth_from_pgrep(output), None);
    }

    #[test]
    fn parse_terminal_auth_from_pgrep_missing_port() {
        let output = "42 ttyd -b /abcdef0123456789abcdef0123456789 -W bash -c ls\n";
        assert_eq!(parse_terminal_auth_from_pgrep(output), None);
    }

    #[test]
    fn parse_terminal_auth_from_pgrep_empty_output() {
        assert_eq!(parse_terminal_auth_from_pgrep(""), None);
    }

    #[test]
    fn parse_terminal_auth_from_pgrep_invalid_port() {
        let output = "42 ttyd -p 99999 -b /aabb1122 bash -c ls\n";
        assert_eq!(parse_terminal_auth_from_pgrep(output), None);
    }

    #[test]
    fn parse_terminal_auth_from_pgrep_multiple_lines() {
        let output =
            "42 ttyd -p 9150 -b /token1 bash -c ls\n99 ttyd -p 9200 -b /token2 bash -c ls\n";
        // Returns the first valid match.
        assert_eq!(
            parse_terminal_auth_from_pgrep(output),
            Some((9150, "token1".to_string()))
        );
    }

    #[test]
    fn parse_terminal_auth_from_pgrep_strips_leading_slash() {
        let output = "42 ttyd -p 9150 -b /mysecrettoken bash -c ls\n";
        let (_, token) = parse_terminal_auth_from_pgrep(output).unwrap();
        assert!(
            !token.starts_with('/'),
            "Token must not start with /: {token}"
        );
        assert_eq!(token, "mysecrettoken");
    }

    #[test]
    fn parse_terminal_auth_from_pgrep_base_path_no_value() {
        // -b is the last argument (no value follows)
        let output = "42 ttyd -p 9150 -b\n";
        assert_eq!(parse_terminal_auth_from_pgrep(output), None);
    }

    #[test]
    fn parse_terminal_auth_from_pgrep_empty_base_path() {
        // -b with just / (empty token after stripping the slash)
        let output = "42 ttyd -p 9150 -b / bash -c ls\n";
        assert_eq!(parse_terminal_auth_from_pgrep(output), None);
    }

    // ── Per-issue volume isolation tests ──────────────────────────────

    /// Helper: create a runner whose worktree path sits under `/workspace/worktrees/`
    /// so the repo root can be derived (parent of parent).
    fn isolated_runner() -> ContainerRunner {
        ContainerRunner::new(
            "PROJ-42",
            &PathBuf::from("/workspace/worktrees/feat-proj-42"),
            "maestro:latest",
        )
        .with_isolate_workspace()
    }

    /// Helper: create a legacy runner (no isolation).
    fn legacy_runner() -> ContainerRunner {
        ContainerRunner::new(
            "PROJ-42",
            &PathBuf::from("/workspace/worktrees/feat-proj-42"),
            "maestro:latest",
        )
    }

    // ── Group 1: Legacy mode (no isolation) ──

    #[test]
    fn legacy_mode_has_workspace_volume() {
        let r = legacy_runner();
        let (_, args) = r.wrap_command("true", &[]);
        assert!(
            has_volume(&args, "/workspace:/workspace"),
            "Legacy mode must mount /workspace:/workspace"
        );
    }

    #[test]
    fn legacy_mode_no_targeted_worktree_mount() {
        let r = legacy_runner();
        let (_, args) = r.wrap_command("true", &[]);
        // No mount of the specific worktree path should appear
        assert!(
            !has_volume(
                &args,
                "/workspace/worktrees/feat-proj-42:/workspace/worktrees/feat-proj-42"
            ),
            "Legacy mode must NOT mount the worktree path separately"
        );
    }

    #[test]
    fn legacy_mode_no_standalone_git_mount() {
        let r = legacy_runner();
        let (_, args) = r.wrap_command("true", &[]);
        assert!(
            !has_volume(&args, "/workspace/.git:/workspace/.git"),
            "Legacy mode must NOT mount .git separately (it is inside /workspace)"
        );
    }

    #[test]
    fn legacy_wrap_shell_command_has_workspace_volume() {
        let r = legacy_runner();
        let (_, args) = r.wrap_shell_command("echo test");
        assert!(
            has_volume(&args, "/workspace:/workspace"),
            "Legacy wrap_shell_command must mount /workspace:/workspace"
        );
    }

    // ── Group 2: Isolated mode ──

    #[test]
    fn isolated_mode_no_full_workspace_mount() {
        let r = isolated_runner();
        let (_, args) = r.wrap_command("true", &[]);
        assert!(
            !has_volume(&args, "/workspace:/workspace"),
            "Isolated mode must NOT mount /workspace:/workspace"
        );
    }

    #[test]
    fn isolated_mode_has_worktree_mount() {
        let r = isolated_runner();
        let (_, args) = r.wrap_command("true", &[]);
        assert!(
            has_volume(
                &args,
                "/workspace/worktrees/feat-proj-42:/workspace/worktrees/feat-proj-42"
            ),
            "Isolated mode must mount the specific worktree path"
        );
    }

    #[test]
    fn isolated_mode_has_git_dir_mount() {
        let r = isolated_runner();
        let (_, args) = r.wrap_command("true", &[]);
        assert!(
            has_volume(&args, "/workspace/.git:/workspace/.git"),
            "Isolated mode must mount .git for git operations"
        );
    }

    #[test]
    fn isolated_mode_has_maestro_dir_mount_ro() {
        let r = isolated_runner();
        let (_, args) = r.wrap_command("true", &[]);
        assert!(
            has_volume(&args, "/workspace/.maestro:/workspace/.maestro:ro"),
            "Isolated mode must mount .maestro read-only for npm config"
        );
    }

    #[test]
    fn isolated_mode_auth_volumes_preserved() {
        let r = isolated_runner();
        let (_, args) = r.wrap_command("true", &[]);
        // All /shared-auth/* mounts must still be present
        for mount in WORKER_VOLUMES {
            if mount.starts_with("/shared-auth/") || mount.starts_with("/etc/maestro") {
                assert!(
                    has_volume(&args, mount),
                    "Isolated mode must preserve auth volume: {mount}"
                );
            }
        }
    }

    #[test]
    fn isolated_mode_env_vars_unchanged() {
        let r = isolated_runner();
        let (_, args) = r.wrap_command("true", &[]);
        for (k, v) in WORKER_ENV {
            assert!(
                has_env(&args, k, v),
                "Isolated mode must preserve env var {k}={v}"
            );
        }
    }

    #[test]
    fn isolated_mode_working_directory_correct() {
        let r = isolated_runner();
        let (_, args) = r.wrap_command("true", &[]);
        assert_eq!(
            flag_value(&args, "-w"),
            Some("/workspace/worktrees/feat-proj-42"),
            "Isolated mode must keep -w pointing to the worktree path"
        );
    }

    #[test]
    fn isolated_wrap_shell_command_no_full_workspace() {
        let r = isolated_runner();
        let (_, args) = r.wrap_shell_command("echo test");
        assert!(
            !has_volume(&args, "/workspace:/workspace"),
            "Isolated wrap_shell_command must NOT mount /workspace:/workspace"
        );
    }

    #[test]
    fn isolated_wrap_shell_command_has_targeted_mounts() {
        let r = isolated_runner();
        let (_, args) = r.wrap_shell_command("echo test");
        assert!(
            has_volume(
                &args,
                "/workspace/worktrees/feat-proj-42:/workspace/worktrees/feat-proj-42"
            ),
            "Isolated wrap_shell_command must mount worktree"
        );
        assert!(
            has_volume(&args, "/workspace/.git:/workspace/.git"),
            "Isolated wrap_shell_command must mount .git"
        );
        assert!(
            has_volume(&args, "/workspace/.maestro:/workspace/.maestro:ro"),
            "Isolated wrap_shell_command must mount .maestro:ro"
        );
    }

    // ── Group 3: Builder API ──

    #[test]
    fn with_isolate_workspace_sets_flag() {
        let r = ContainerRunner::new(
            "TEST-1",
            &PathBuf::from("/workspace/worktrees/test-1"),
            "maestro:latest",
        )
        .with_isolate_workspace();
        let (_, args) = r.wrap_command("true", &[]);
        assert!(
            !has_volume(&args, "/workspace:/workspace"),
            "with_isolate_workspace must enable isolation"
        );
    }

    #[test]
    fn default_runner_no_isolation() {
        let r = ContainerRunner::new(
            "TEST-1",
            &PathBuf::from("/workspace/worktrees/test-1"),
            "maestro:latest",
        );
        let (_, args) = r.wrap_command("true", &[]);
        assert!(
            has_volume(&args, "/workspace:/workspace"),
            "Default runner must NOT isolate (backward compat)"
        );
    }

    #[test]
    fn isolate_workspace_active() {
        let r = ContainerRunner::new(
            "TEST-1",
            &PathBuf::from("/workspace/worktrees/test-1"),
            "maestro:latest",
        )
        .with_isolate_workspace();
        let (_, args) = r.wrap_command("true", &[]);
        // Isolation must be active
        assert!(
            !has_volume(&args, "/workspace:/workspace"),
            "Isolation must be active"
        );
        assert!(
            has_volume(
                &args,
                "/workspace/worktrees/test-1:/workspace/worktrees/test-1"
            ),
            "Worktree mount must be present"
        );
    }

    #[test]
    fn wrap_command_sources_gh_token_file() {
        let r = runner();
        let (_, args) = r.wrap_command("claude", &["--print"]);
        let sh_body = args.last().expect("last arg is the sh -c body");
        assert!(
            sh_body.contains("gh-app-token"),
            "wrap_command preamble must source the GitHub App token file; got: {sh_body}"
        );
    }

    // ── Group 4: build_volume_args helper tests ──

    #[test]
    fn build_volume_args_legacy_includes_workspace() {
        let wt = PathBuf::from("/workspace/worktrees/feat-proj-42");
        let args = build_volume_args(&wt, false);
        let pairs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        assert!(
            pairs.contains(&"/workspace:/workspace"),
            "Legacy build_volume_args must include /workspace:/workspace"
        );
    }

    #[test]
    fn build_volume_args_isolated_replaces_workspace() {
        let wt = PathBuf::from("/workspace/worktrees/feat-proj-42");
        let args = build_volume_args(&wt, true);
        let pairs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        assert!(
            !pairs.contains(&"/workspace:/workspace"),
            "Isolated build_volume_args must NOT include /workspace:/workspace"
        );
        assert!(
            pairs.contains(&"/workspace/worktrees/feat-proj-42:/workspace/worktrees/feat-proj-42"),
            "Isolated build_volume_args must include worktree mount"
        );
        assert!(
            pairs.contains(&"/workspace/.git:/workspace/.git"),
            "Isolated build_volume_args must include .git mount"
        );
        assert!(
            pairs.contains(&"/workspace/.maestro:/workspace/.maestro:ro"),
            "Isolated build_volume_args must include .maestro:ro mount"
        );
    }

    #[test]
    fn build_volume_args_isolated_no_duplicate_mounts() {
        let wt = PathBuf::from("/workspace/worktrees/feat-proj-42");
        let args = build_volume_args(&wt, true);
        // Check for duplicate entries
        let mut seen = std::collections::HashSet::new();
        for mount in &args {
            assert!(
                seen.insert(mount.as_str()),
                "Duplicate volume mount: {mount}"
            );
        }
    }

    #[test]
    fn build_volume_args_isolated_shallow_path_falls_back() {
        // A shallow path like `/tmp` has no grandparent, so isolation cannot
        // derive the repo root. The function should fall back to the full
        // `/workspace:/workspace` mount instead of leaving the container
        // without any workspace volume.
        let wt = PathBuf::from("/tmp");
        let args = build_volume_args(&wt, true);
        let pairs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        assert!(
            pairs.contains(&"/workspace:/workspace"),
            "Shallow worktree path must fall back to /workspace:/workspace"
        );
    }
}
