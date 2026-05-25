// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! openvscode-server editor container lifecycle, plus the URL / token
//! helpers used by the shared-port reverse proxy (`/s/<token>/…`).

use std::path::Path;

use tracing::{info, warn};

use super::runner::{
    PASSTHROUGH_ENV, WORKER_ENV, apply_secrets_bundle_to_args, build_volume_args,
    passthrough_is_bundled,
};
use super::{editor_host_port, is_dind_mode, sanitize_ticket_key, shell_escape};

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
pub(crate) const EDITOR_PORT_MIN: u16 = 9100;
pub(crate) const EDITOR_PORT_MAX: u16 = 19100;

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
pub(crate) fn editor_container_name(ticket_key: &str) -> String {
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

/// In-memory set of allocated ports. Eliminates the race condition where
/// two concurrent `allocate_editor_ports` calls both query Docker labels
/// before either's container has started, causing both to get the same ports.
static ALLOCATED_PORTS: std::sync::LazyLock<tokio::sync::Mutex<std::collections::HashSet<u16>>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(std::collections::HashSet::new()));

/// Allocate `count` free host ports from the editor range.
///
/// Uses an in-memory set (no Docker label race) plus a Docker query as a
/// secondary check for ports allocated by a previous Maestro process.
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
    // SAFETY: `getrandom::fill` only fails when the OS CSPRNG is
    // unavailable. See the function-level doc comment above for the full
    // rationale — token reuse is unacceptable, so panicking is correct.
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

/// Build a relative proxy URL for a dynamically forwarded application port.
///
/// Returns `/s/<path-token>/`. Unlike editor/terminal URLs there is no
/// secondary in-process auth token — the path token (validated by the proxy
/// registry) and the session cookie are the two access-control layers.
pub fn build_session_dynamic_port_url(path_token: &str) -> String {
    format!("/s/{path_token}/")
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
    dynamic_ports: usize,
    theme: &str,
    extensions: &[String],
    settings: &std::collections::HashMap<String, toml::Value>,
    setup_commands: &[String],
    startup_commands: &[String],
    git_editor: &str,
    isolate_workspace: bool,
    // Phase 2b.3.x: optional per-workflow secrets bundle. When `Some`,
    // secret PASSTHROUGH names are suppressed and the bundle's tmpfs
    // directory is bind-mounted at `/run/maestro-secrets:ro`. Token bytes
    // reach the editor's in-browser terminal via the file path, not
    // `docker inspect`.
    secrets_bundle: Option<&crate::auth::WorkerSecretsBundle>,
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

    // Allocate 1 port for VS Code + N spare ports for dynamic port forwarding
    // (dev servers started by the user inside the container).
    let total_ports = 1 + dynamic_ports;
    let ports = allocate_editor_ports(total_ports).await.ok_or_else(|| {
        format!("No free editor ports available (range {EDITOR_PORT_MIN}–{EDITOR_PORT_MAX})")
    })?;

    let vscode_port = ports[0];
    let spare_ports: Vec<u16> = ports[1..].to_vec();
    let port_mappings: Vec<(u16, u16)> = Vec::new();
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

    // Volumes — use per-issue isolation when enabled
    for mount in build_volume_args(worktree_path, isolate_workspace) {
        args.push("-v".into());
        args.push(mount);
    }

    // Phase 2b.3.x: attach the bundle AFTER the standard volumes so the
    // bundle's `-v` and `-e` flags are colocated.
    if let Some(b) = secrets_bundle {
        apply_secrets_bundle_to_args(&mut args, b);
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
        let sp_csv: String = spare_ports
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join(",");
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
    // Release allocated ports before removing the container.
    release_container_ports(&name).await;
    let _ = tokio::process::Command::new("docker")
        .args(["rm", "-f", &name])
        .output()
        .await;
    info!(name = %name, "Editor container stopped");
}

/// Query a container's `maestro.vscode_port` and `maestro.spare_ports`
/// labels and release them from the in-memory allocation set.
pub(crate) async fn release_container_ports(container_name: &str) {
    let output = tokio::process::Command::new("docker")
        .args([
            "inspect",
            "--format",
            "{{index .Config.Labels \"maestro.vscode_port\"}} {{index .Config.Labels \"maestro.spare_ports\"}}",
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
    let path_token = parse_label_value(labels_json, "maestro.path_token").unwrap_or_default();

    // Port discovery: prefer labels (works for both `--network=host` and
    // `-p` containers) with `docker port` as fallback for pre-label containers.
    let (vscode_port, spare_ports, port_mappings) = if let Some(vp_str) =
        parse_label_value(labels_json, "maestro.vscode_port")
    {
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
            if parts.len() != 2 {
                continue;
            }
            let cp: u16 = parts[0]
                .trim()
                .split('/')
                .next()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let hp: u16 = parts[1]
                .trim()
                .rsplit(':')
                .next()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            if cp == 0 || hp == 0 {
                continue;
            }
            if (EDITOR_PORT_MIN..=EDITOR_PORT_MAX).contains(&cp) && cp == hp {
                if !erp.contains(&hp) {
                    erp.push(hp);
                }
            } else if !(EDITOR_PORT_MIN..=EDITOR_PORT_MAX).contains(&cp) && !pm.contains(&(cp, hp))
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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(
            url,
            "/s/tok/?tkn=conn&folder=/workspace/my%20project%26foo%23bar"
        );
    }

    #[test]
    fn encode_query_value_preserves_slashes_and_alphanumerics() {
        assert_eq!(
            encode_query_value("/workspace/proj-name"),
            "/workspace/proj-name"
        );
    }

    #[test]
    fn encode_query_value_encodes_query_unsafe_chars() {
        assert_eq!(
            encode_query_value("a&b=c#d+e f%g"),
            "a%26b%3dc%23d%2be%20f%25g"
        );
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
}
