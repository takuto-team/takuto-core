use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::workflow::engine::WorkflowEvent;

/// Shell-escape a string for safe inclusion in `sh -c "..."`.
fn shell_escape(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    // If the string is safe (alphanumeric, common flags), return as-is.
    if s.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'/' || b == b'.' || b == b'=' || b == b':')
    {
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

/// Runs AI agent commands inside isolated Docker containers so each workflow
/// gets its own filesystem and network namespace.
pub struct ContainerRunner {
    ticket_key: String,
    image: String,
    worktree_path: PathBuf,
    step_counter: std::sync::atomic::AtomicU32,
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
        "/home/maestro/.local/share/mise/shims:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
    ),
    ("DOCKER_HOST", "tcp://dind:2375"),
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
    "CURSOR_API_KEY",
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

impl ContainerRunner {
    pub fn new(ticket_key: &str, worktree_path: &Path, image: &str) -> Self {
        Self {
            ticket_key: ticket_key.to_string(),
            image: image.to_string(),
            worktree_path: worktree_path.to_path_buf(),
            step_counter: std::sync::atomic::AtomicU32::new(0),
        }
    }

    /// Check if Docker is available (`DOCKER_HOST` set and `docker info` succeeds).
    /// The result is cached for the process lifetime.
    pub fn is_available() -> bool {
        *DOCKER_AVAILABLE.get_or_init(|| {
            if std::env::var("DOCKER_HOST").unwrap_or_default().is_empty() {
                info!("DOCKER_HOST not set — container isolation disabled");
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
                warn!("docker info failed — container isolation disabled");
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
            if let Ok(val) = std::env::var(key) {
                if !val.is_empty() {
                    args.push("-e".into());
                    args.push(format!("{key}={val}"));
                }
            }
        }

        for v in WORKER_VOLUMES {
            args.push("-v".into());
            args.push((*v).into());
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
        let cmd = format!("{restore}; exec {}", shell_parts.join(" "));
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

    /// Auto-detect the worker image by inspecting the running Maestro container.
    pub async fn discover_worker_image() -> Option<String> {
        let output = tokio::process::Command::new("docker")
            .args(["inspect", "maestro", "--format", "{{.Config.Image}}"])
            .output()
            .await
            .ok()?;

        if !output.status.success() {
            warn!("docker inspect maestro failed — cannot auto-detect worker image");
            return None;
        }

        let image = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if image.is_empty() {
            None
        } else {
            info!(image = %image, "Discovered worker image from running Maestro container");
            Some(image)
        }
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
const EDITOR_PORT_MAX: u16 = 9200;

/// Information about a running editor container.
#[derive(Debug, Clone, serde::Serialize)]
pub struct EditorInfo {
    /// URL to open in the browser (e.g. `http://localhost:9100/?folder=...`).
    pub url: String,
    /// VS Code port on the host.
    pub vscode_port: u16,
    /// `(container_port, host_port)` pairs for user-configured application ports.
    pub port_mappings: Vec<(u16, u16)>,
    /// Pre-allocated spare host ports for dynamic forwarding (socat-based).
    #[serde(default)]
    pub spare_ports: Vec<u16>,
}

/// Return the deterministic editor container name for a ticket.
fn editor_container_name(ticket_key: &str) -> String {
    format!("maestro-editor-{}", sanitize_ticket_key(ticket_key))
}

/// List host ports already claimed by any `maestro-editor-*` container.
async fn used_editor_ports() -> Vec<u16> {
    let output = tokio::process::Command::new("docker")
        .args(["ps", "--filter", "name=maestro-editor-", "--format", "{{.Ports}}"])
        .output()
        .await;

    let Ok(out) = output else { return Vec::new() };
    if !out.status.success() {
        return Vec::new();
    }

    let stdout = String::from_utf8_lossy(&out.stdout);
    let mut ports = Vec::new();
    // Format: "0.0.0.0:9100->9100/tcp, 0.0.0.0:9101->3000/tcp"
    for segment in stdout.split(|c: char| c == ',' || c == '\n') {
        let segment = segment.trim();
        // Extract host port from "0.0.0.0:PORT->"
        if let Some(arrow) = segment.find("->") {
            if let Some(colon) = segment[..arrow].rfind(':') {
                if let Ok(p) = segment[colon + 1..arrow].parse::<u16>() {
                    ports.push(p);
                }
            }
        }
    }
    ports
}

/// Allocate `count` free host ports from the editor range.
async fn allocate_editor_ports(count: usize) -> Option<Vec<u16>> {
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
    None // not enough free ports
}

/// Start a browser VS Code editor container for a workflow.
///
/// Returns [`EditorInfo`] with the URL and port mappings on success.
#[allow(clippy::too_many_arguments)]
pub async fn start_editor(
    ticket_key: &str,
    worktree_path: &Path,
    image: &str,
    app_ports: &[u16],
    dynamic_ports: usize,
    theme: &str,
    extensions: &[String],
    settings: &std::collections::HashMap<String, toml::Value>,
    setup_commands: &[String],
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

    // Allocate ports: 1 for VS Code + N for app ports + M spare for dynamic forwarding
    let needed = 1 + app_ports.len() + dynamic_ports;
    let ports = allocate_editor_ports(needed).await.ok_or_else(|| {
        format!("No free editor ports available (need {needed}, range {EDITOR_PORT_MIN}–{EDITOR_PORT_MAX})")
    })?;

    let vscode_port = ports[0];
    let mut port_mappings = Vec::new();
    let spare_start = 1 + app_ports.len();
    let spare_ports: Vec<u16> = ports[spare_start..].to_vec();

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
        if let Ok(val) = std::env::var(key) {
            if !val.is_empty() {
                args.push("-e".into());
                args.push(format!("{key}={val}"));
            }
        }
    }

    // Volumes
    for v in WORKER_VOLUMES {
        args.push("-v".into());
        args.push((*v).into());
    }

    // Port mappings: VS Code
    args.push("-p".into());
    args.push(format!("{vscode_port}:{vscode_port}"));

    // Port mappings: app ports
    for (i, &app_port) in app_ports.iter().enumerate() {
        let host_port = ports[1 + i];
        args.push("-p".into());
        args.push(format!("{host_port}:{app_port}"));
        port_mappings.push((app_port, host_port));
    }

    // Port mappings: spare ports for dynamic forwarding (symmetric — socat listens inside container)
    for &sp in &spare_ports {
        args.push("-p".into());
        args.push(format!("{sp}:{sp}"));
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
        script_parts.push(format!("openvscode-server --install-extension {escaped} --force 2>/dev/null || true"));
    }

    script_parts.push(format!(
        "exec openvscode-server --port {vscode_port} --host 0.0.0.0 --without-connection-token"
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

    // Run setup commands as root inside the new container (tool installs, etc.).
    // Runs once per container lifetime (gated by /tmp/.maestro-terminal-setup-done).
    if !setup_commands.is_empty() {
        run_editor_setup_as_root(&name, setup_commands).await;
    }

    let url = format!("http://localhost:{vscode_port}/?folder={folder}");
    info!(url = %url, spare = ?spare_ports, "Editor container started");

    Ok(EditorInfo {
        url,
        vscode_port,
        port_mappings,
        spare_ports,
    })
}

/// Run setup commands inside the editor container as root. Ensures ownership of the
/// maestro user's home and cache directories is correct so `mise install` etc. can write.
/// Idempotent via `/tmp/.maestro-terminal-setup-done` marker.
async fn run_editor_setup_as_root(container: &str, setup_commands: &[String]) {
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
        .args(["exec", "--user", "root", container, "bash", "-lc", chown_script])
        .output()
        .await;

    let joined = setup_commands.join(" && echo && ");
    let wrapped = format!(
        "[ -f /etc/maestro/env ] && set -a && . /etc/maestro/env && set +a; {joined} && mise reshim 2>&1 || true"
    );
    // Run setup commands as the maestro user so tools are installed under their home dir,
    // not root's. Use `su - maestro -c` from the root exec context.
    let out = tokio::process::Command::new("docker")
        .args([
            "exec", "--user", "root", container, "bash", "-lc",
            &format!(r#"su - maestro -c {}"#, shell_escape(&wrapped)),
        ])
        .output()
        .await;

    match out {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            info!(container, %stdout, "Editor setup commands completed");
            // Mark done so subsequent calls skip.
            let _ = tokio::process::Command::new("docker")
                .args(["exec", container, "touch", marker])
                .output()
                .await;
        }
        Ok(o) => {
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
        Err(e) => {
            warn!(container, error = %e, "Editor setup docker exec errored (continuing)");
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
) -> std::result::Result<String, String> {
    let name = editor_container_name(ticket_key);

    // Check the editor container is actually running.
    if get_editor_info(ticket_key).await.is_none() {
        return Err("Editor container is not running — open the editor first.".into());
    }

    // Check if ttyd is already running inside the container.
    let check = tokio::process::Command::new("docker")
        .args(["exec", &name, "pgrep", "-x", "ttyd"])
        .output()
        .await;
    if let Ok(out) = &check {
        if out.status.success() {
            // Already running — return URL with the given port.
            return Ok(format!("http://localhost:{port}"));
        }
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
    let output = tokio::process::Command::new("docker")
        .args([
            "exec", "-d", &name, "ttyd", "-p",
            &port.to_string(), "-W", "-t", "fontSize=14",
            "bash", "-c", &shell_cmd,
        ])
        .output()
        .await
        .map_err(|e| format!("Failed to start ttyd: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("ttyd start failed: {stderr}"));
    }

    let url = format!("http://localhost:{port}");
    info!(ticket = %ticket_key, url = %url, "Web terminal started");
    Ok(url)
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

    let output = tokio::process::Command::new("docker")
        .args(["inspect", &name, "--format", "{{.State.Running}} {{range .HostConfig.PortBindings}}{{range .}}{{.HostPort}} {{end}}{{end}} {{json .Config.Labels}}"])
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    // Simpler approach: get ports via docker port
    let port_output = tokio::process::Command::new("docker")
        .args(["port", &name])
        .output()
        .await
        .ok()?;

    if !port_output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&port_output.stdout);
    let mut port_mappings = Vec::new();
    // Symmetric mappings where both ports are in the editor range (VS Code + spare ports).
    let mut editor_range_ports: Vec<u16> = Vec::new();

    // Format: "9100/tcp -> 0.0.0.0:9100\n3000/tcp -> 0.0.0.0:9101"
    for line in stdout.lines() {
        let parts: Vec<&str> = line.split("->").collect();
        if parts.len() != 2 {
            continue;
        }
        let container_part = parts[0].trim(); // "9100/tcp"
        let host_part = parts[1].trim(); // "0.0.0.0:9100"

        let container_port: u16 = container_part
            .split('/')
            .next()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let host_port: u16 = host_part
            .rsplit(':')
            .next()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        if container_port == 0 || host_port == 0 {
            continue;
        }

        // Symmetric mapping in editor range → VS Code or spare port.
        // `docker port` may emit the same mapping twice (IPv4 + IPv6), so dedupe.
        if (EDITOR_PORT_MIN..=EDITOR_PORT_MAX).contains(&container_port)
            && container_port == host_port
        {
            if !editor_range_ports.contains(&host_port) {
                editor_range_ports.push(host_port);
            }
        } else if !(EDITOR_PORT_MIN..=EDITOR_PORT_MAX).contains(&container_port) {
            // Asymmetric: app port (container) → host port
            if !port_mappings.contains(&(container_port, host_port)) {
                port_mappings.push((container_port, host_port));
            }
        }
    }

    editor_range_ports.sort();
    // The lowest port in the editor range is VS Code; the rest are spare/dynamic.
    let vscode_port = *editor_range_ports.first()?;
    let spare_ports: Vec<u16> = editor_range_ports.into_iter().skip(1).collect();

    // Get the working directory to reconstruct the folder URL
    let wd_output = tokio::process::Command::new("docker")
        .args(["inspect", &name, "--format", "{{.Config.WorkingDir}}"])
        .output()
        .await
        .ok()?;

    let folder = String::from_utf8_lossy(&wd_output.stdout).trim().to_string();
    let url = format!("http://localhost:{vscode_port}/?folder={folder}");

    Some(EditorInfo {
        url,
        vscode_port,
        port_mappings,
        spare_ports,
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

/// Scan listening ports inside an editor container and set up socat forwarding
/// from pre-allocated spare host ports. Runs until `cancel` is triggered.
///
/// `known_ports` contains the VS Code port and any configured app ports — these
/// are ignored by the scanner. `spare_ports` is the pool of symmetric-mapped
/// ports available for dynamic forwarding.
pub async fn run_port_scanner(
    ticket_key: &str,
    vscode_port: u16,
    app_ports: &[u16],
    spare_ports: Vec<u16>,
    event_tx: broadcast::Sender<WorkflowEvent>,
    cancel: CancellationToken,
) {
    let container = editor_container_name(ticket_key);
    let ticket = ticket_key.to_string();

    // Ports to never forward (VS Code, configured app ports, spare ports themselves).
    let mut ignore: Vec<u16> = vec![vscode_port];
    ignore.extend_from_slice(app_ports);
    ignore.extend_from_slice(&spare_ports);

    // active_forwards: detected_port → spare_port
    let mut active_forwards: HashMap<u16, u16> = HashMap::new();
    let mut available_spares: Vec<u16> = spare_ports;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                // Kill all socat processes before exiting.
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
            None => continue, // container might be starting up or gone
        };

        // Detect new ports.
        for &port in &listening {
            if ignore.contains(&port) || active_forwards.contains_key(&port) {
                continue;
            }
            // Also ignore ports used by our own socat listeners.
            if active_forwards.values().any(|&sp| sp == port) {
                continue;
            }
            if let Some(spare) = available_spares.pop() {
                if start_socat(&container, spare, port).await {
                    info!(
                        ticket = %ticket,
                        detected = port,
                        host_port = spare,
                        "Dynamic port forwarded via socat"
                    );
                    active_forwards.insert(port, spare);
                    ignore.push(spare); // don't re-scan the socat listener

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
                        forwarded_port: Some((port, spare)),
                    });
                } else {
                    // socat failed — return spare to pool.
                    available_spares.push(spare);
                }
            } else {
                warn!(
                    ticket = %ticket,
                    port,
                    "No spare ports left for dynamic forwarding"
                );
            }
        }

        // Detect removed ports.
        let gone: Vec<u16> = active_forwards
            .keys()
            .copied()
            .filter(|p| !listening.contains(p))
            .collect();

        for port in gone {
            if let Some(spare) = active_forwards.remove(&port) {
                kill_socat(&container, spare).await;
                ignore.retain(|&p| p != spare);
                available_spares.push(spare);
                info!(
                    ticket = %ticket,
                    detected = port,
                    host_port = spare,
                    "Dynamic port forward removed"
                );

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
                    forwarded_port: Some((port, spare)),
                });
            }
        }
    }
}

/// Run `ss -tlnH` inside the container and return listening ports.
async fn scan_listening_ports(container: &str) -> Option<Vec<u16>> {
    let output = tokio::process::Command::new("docker")
        .args(["exec", container, "ss", "-tlnH"])
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut ports = Vec::new();
    // Format: "LISTEN  0  128  0.0.0.0:6006  0.0.0.0:*"
    //    or:  "LISTEN  0  128  [::]:6006     [::]:*"
    for line in stdout.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        // The local address is typically the 4th field (index 3).
        if fields.len() >= 4 {
            let local = fields[3];
            if let Some(port_str) = local.rsplit(':').next() {
                if let Ok(port) = port_str.parse::<u16>() {
                    if port > 0 && !ports.contains(&port) {
                        ports.push(port);
                    }
                }
            }
        }
    }
    Some(ports)
}

/// Start a `socat` process inside the container to forward `spare_port` → `target_port`.
async fn start_socat(container: &str, spare_port: u16, target_port: u16) -> bool {
    let output = tokio::process::Command::new("docker")
        .args([
            "exec",
            "-d",
            container,
            "socat",
            &format!("TCP-LISTEN:{spare_port},fork,reuseaddr"),
            &format!("TCP:localhost:{target_port}"),
        ])
        .output()
        .await;

    match output {
        Ok(o) if o.status.success() => true,
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
    let pattern = format!("TCP-LISTEN:{spare_port}");
    let _ = tokio::process::Command::new("docker")
        .args(["exec", container, "pkill", "-f", &pattern])
        .output()
        .await;
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
        assert!(has_env(&args, "DOCKER_HOST", "tcp://dind:2375"));
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
        assert!(tail[5].contains("exec claude --print -p hello"), "sh -c body: {}", tail[5]);
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
}
