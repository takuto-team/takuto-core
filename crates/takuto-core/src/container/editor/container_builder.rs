// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! `docker run` orchestration for openvscode-server containers, plus the
//! `EditorInfo` public type returned to the dashboard. Lifecycle:
//! [`start_editor`] → [`get_editor_info`] (idempotent reconnect) →
//! [`stop_editor`].

use std::path::Path;

use tracing::{info, warn};

use super::super::workspace::{
    WorkspaceStatus, ensure_workspace_container, workspace_container_name, workspace_status,
};
use super::super::{editor_host_port, shell_escape};
use super::port_alloc::{EDITOR_PORT_MAX, EDITOR_PORT_MIN, allocate_editor_ports};
use super::token_gen::{generate_connection_token, generate_session_path_token};
use super::urls::build_editor_url;

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
    /// a container label (`takuto.path_token`) so `get_editor_info` can
    /// return the same token on reconnect, keeping `--server-base-path` in
    /// sync with the proxy registry.
    pub path_token: String,
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
    // Optional per-workflow secrets bundle. When `Some`, secret
    // PASSTHROUGH names are suppressed and the bundle's tmpfs directory
    // is bind-mounted at `/run/takuto-secrets:ro`. Token bytes reach the
    // editor's in-browser terminal via the file path, not `docker
    // inspect`.
    secrets_bundle: Option<&crate::auth::WorkerSecretsBundle>,
    // Workspace init commands — run when the workspace container is brought up.
    init_commands: &[String],
) -> std::result::Result<EditorInfo, String> {
    // Idempotent: if the IDE is already running in the workspace container,
    // return its existing session info.
    if let Some(info) = get_editor_info(ticket_key).await {
        return Ok(info);
    }

    let name = workspace_container_name(ticket_key);

    // Reuse the workspace container's already-published ports when it is
    // already up (e.g. a run-command created it, or the IDE was closed but the
    // container persists); otherwise allocate a fresh vscode + spare set.
    let ports: Vec<u16> = match read_ws_spare_ports(&name).await {
        Some(p) if !p.is_empty() => p,
        _ => {
            let total_ports = 1 + dynamic_ports;
            allocate_editor_ports(total_ports).await.ok_or_else(|| {
                format!(
                    "No free editor ports available (range {EDITOR_PORT_MIN}–{EDITOR_PORT_MAX})"
                )
            })?
        }
    };
    let vscode_port = ports[0];
    let spare_ports: Vec<u16> = ports[1..].to_vec();
    let connection_token = generate_connection_token();
    let path_token = generate_session_path_token();
    info!(ticket = %name, vscode_port, "Editor session ports");

    // Ensure the per-item workspace container exists and is running. It owns
    // the env / volumes / secrets-bundle / port publishing; the IDE is just a
    // process we exec into it.
    ensure_workspace_container(
        ticket_key,
        worktree_path,
        image,
        isolate_workspace,
        secrets_bundle,
        &ports,
        init_commands,
    )
    .await
    .map_err(|e| format!("Failed to ensure workspace container: {e}"))?;

    // One-time setup (apt installs, git editor) — gated by a marker file — plus
    // per-open startup commands.
    if !setup_commands.is_empty() || !git_editor.is_empty() {
        run_editor_setup_as_root(&name, setup_commands, git_editor).await;
    }
    run_editor_startup_commands(&name, startup_commands).await;

    // Build settings.json content from theme + settings map.
    let folder = worktree_path.to_string_lossy();
    let mut settings_json = serde_json::Map::new();
    if !theme.is_empty() {
        settings_json.insert(
            "workbench.colorTheme".into(),
            serde_json::Value::String(theme.to_string()),
        );
    }
    settings_json.insert(
        "window.title".into(),
        serde_json::Value::String(format!(
            "{ticket_key} — ${{activeEditorShort}}${{separator}}${{rootName}}"
        )),
    );
    for (key, val) in settings {
        settings_json.insert(key.clone(), toml_value_to_json(val));
    }

    // Shell script: source env, write settings, install extensions, then exec
    // openvscode-server. Run detached (`docker exec -d`) inside the workspace
    // container — the exec'd process IS the editor (recoverable via pgrep).
    let mut script_parts: Vec<String> = Vec::new();
    script_parts.push("[ -f /etc/takuto/env ] && set -a && . /etc/takuto/env && set +a".into());
    if !settings_json.is_empty() {
        let json_str = serde_json::to_string(&settings_json).unwrap_or_default();
        let escaped = json_str.replace('\'', "'\\''");
        script_parts.push(format!(
            "mkdir -p ~/.openvscode-server/data/Machine && echo '{escaped}' > ~/.openvscode-server/data/Machine/settings.json"
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

    let output = tokio::process::Command::new("docker")
        .args(["exec", "-d", &name, "sh", "-c", &cmd])
        .output()
        .await
        .map_err(|e| format!("Failed to launch editor in workspace container: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "openvscode-server launch failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    // Wait until openvscode-server is actually accepting connections before
    // handing back the URL. `docker exec -d` returns the instant the process is
    // spawned, but the IDE needs a moment to bind its port; without this wait
    // the browser hits the session proxy first and gets "upstream unavailable".
    // Probe `/dev/tcp` inside the container (same approach as the web terminal),
    // which works regardless of DinD network topology. Best-effort: on timeout
    // we still return the URL and let the browser retry.
    for attempt in 0..40 {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        let probe = tokio::process::Command::new("docker")
            .args([
                "exec",
                &name,
                "bash",
                "-c",
                &format!("echo > /dev/tcp/127.0.0.1/{vscode_port}"),
            ])
            .output()
            .await;
        if matches!(probe, Ok(ref o) if o.status.success()) {
            break;
        }
        if attempt == 39 {
            tracing::warn!(
                container = %name,
                vscode_port,
                "openvscode-server not listening after wait budget — returning URL anyway (browser will retry)"
            );
        }
    }

    let host_vscode_port = editor_host_port(vscode_port);
    let url = build_editor_url(host_vscode_port, &connection_token, &folder);
    info!(spare = ?spare_ports, "Editor launched in workspace container");

    Ok(EditorInfo {
        url,
        connection_token,
        vscode_port,
        port_mappings: Vec::new(),
        spare_ports,
        folder: folder.into_owned(),
        path_token,
    })
}

/// Read the workspace container's `takuto.spare_ports` label as a port list,
/// or `None` if the container is absent or the label is unset.
async fn read_ws_spare_ports(name: &str) -> Option<Vec<u16>> {
    let out = tokio::process::Command::new("docker")
        .args([
            "inspect",
            name,
            "--format",
            "{{index .Config.Labels \"takuto.spare_ports\"}}",
        ])
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    let s = s.trim();
    if s.is_empty() || s == "<no value>" {
        return None;
    }
    let ports: Vec<u16> = s.split(',').filter_map(|t| t.trim().parse().ok()).collect();
    if ports.is_empty() { None } else { Some(ports) }
}

/// Parse openvscode-server's `--port`, `--connection-token`, and
/// `--server-base-path /s/<token>` from a `pgrep -af openvscode-server` line.
/// Returns `None` if any is missing — mirrors `terminal::parse_terminal_auth_from_pgrep`
/// so the IDE session is recoverable label-free and across restarts.
pub(crate) fn parse_editor_from_pgrep(pgrep_output: &str) -> Option<(u16, String, String)> {
    pgrep_output.lines().find_map(|line| {
        let parts: Vec<&str> = line.split_whitespace().collect();
        let port = parts
            .windows(2)
            .find(|w| w[0] == "--port")
            .and_then(|w| w[1].parse::<u16>().ok())?;
        let token = parts
            .windows(2)
            .find(|w| w[0] == "--connection-token")
            .map(|w| w[1].to_string())?;
        let base = parts
            .windows(2)
            .find(|w| w[0] == "--server-base-path")
            .map(|w| w[1])?;
        let path_token = base.strip_prefix("/s/")?;
        if token.is_empty() || path_token.is_empty() {
            return None;
        }
        Some((port, token, path_token.to_string()))
    })
}

/// Run setup commands inside the editor container as root. Ensures ownership of the
/// takuto user's home and cache directories is correct so `mise install` etc. can write.
/// Idempotent via `/tmp/.takuto-terminal-setup-done` marker.
async fn run_editor_setup_as_root(container: &str, setup_commands: &[String], git_editor: &str) {
    // Skip if already done for this container.
    let marker = "/tmp/.takuto-terminal-setup-done";
    let marker_check = tokio::process::Command::new("docker")
        .args(["exec", container, "test", "-f", marker])
        .output()
        .await;
    if matches!(&marker_check, Ok(o) if o.status.success()) {
        return;
    }

    info!(container, "Running editor setup commands as root");

    // Ensure takuto owns its home dir & mise volumes (fresh volumes mount root-owned).
    // Runs as root unconditionally; fast and safe.
    let chown_script = r#"
chown -R takuto:takuto /home/takuto/.local/share/mise /home/takuto/.cache/mise 2>/dev/null || true
chown -R takuto:takuto /home/takuto/.config/mise 2>/dev/null || true
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
             && su - takuto -c 'git config --global core.editor {escaped}'"
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
            "su - takuto -c 'gh auth setup-git 2>/dev/null || true'",
        ])
        .output()
        .await;

    // Run user-defined setup_commands as the takuto user.
    let out = if !setup_commands.is_empty() {
        let joined = setup_commands.join(" && echo && ");
        let wrapped = format!(
            "[ -f /etc/takuto/env ] && set -a && . /etc/takuto/env && set +a; {joined} && mise reshim 2>&1 || true"
        );
        tokio::process::Command::new("docker")
            .args([
                "exec",
                "--user",
                "root",
                container,
                "bash",
                "-lc",
                &format!(r#"su - takuto -c {}"#, shell_escape(&wrapped)),
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

/// Run `startup_commands` as the takuto user inside the editor container.
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
        "[ -f /etc/takuto/env ] && set -a && . /etc/takuto/env && set +a; {joined} && mise reshim 2>&1 || true"
    );
    let out = tokio::process::Command::new("docker")
        .args([
            "exec",
            "--user",
            "root",
            container,
            "bash",
            "-lc",
            &format!("su - takuto -c {}", shell_escape(&wrapped)),
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

/// Stop the IDE for a workflow. Kills just the openvscode-server process; the
/// per-item workspace container persists for the terminal and run-commands.
/// Container removal happens when the item leaves the dashboard
/// (`ContainerRunner::cleanup_for_ticket`).
pub async fn stop_editor(ticket_key: &str) {
    let name = workspace_container_name(ticket_key);
    let _ = tokio::process::Command::new("docker")
        .args(["exec", &name, "pkill", "-f", "openvscode-server"])
        .output()
        .await;
    info!(name = %name, "Editor process stopped (workspace container persists)");
}

/// Return the running IDE's info, or `None` if the workspace container isn't
/// running or openvscode-server isn't alive in it. The session (port + tokens)
/// is recovered from the process cmdline via `pgrep`, so it survives a Takuto
/// restart without depending on container labels.
pub async fn get_editor_info(ticket_key: &str) -> Option<EditorInfo> {
    if workspace_status(ticket_key).await != WorkspaceStatus::Running {
        return None;
    }
    let name = workspace_container_name(ticket_key);

    let out = tokio::process::Command::new("docker")
        .args(["exec", &name, "pgrep", "-af", "openvscode-server"])
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None; // IDE process not running in the container.
    }
    let (vscode_port, connection_token, path_token) =
        parse_editor_from_pgrep(&String::from_utf8_lossy(&out.stdout))?;

    // Spare ports come from the container label, minus the vscode port.
    let spare_ports: Vec<u16> = read_ws_spare_ports(&name)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|&p| p != vscode_port)
        .collect();

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
        port_mappings: Vec::new(),
        spare_ports,
        folder,
        path_token,
    })
}

#[cfg(test)]
mod tests {
    use super::parse_editor_from_pgrep;

    #[test]
    fn parse_editor_from_pgrep_extracts_port_and_tokens() {
        let line = "57 openvscode-server --port 9100 --host 0.0.0.0 \
                    --connection-token abcdef0123456789 --server-base-path /s/deadbeef00112233\n";
        assert_eq!(
            parse_editor_from_pgrep(line),
            Some((
                9100,
                "abcdef0123456789".to_string(),
                "deadbeef00112233".to_string()
            ))
        );
    }

    #[test]
    fn parse_editor_from_pgrep_handles_reordered_flags() {
        let line =
            "57 openvscode-server --server-base-path /s/tok --connection-token conn --port 9201\n";
        assert_eq!(
            parse_editor_from_pgrep(line),
            Some((9201, "conn".to_string(), "tok".to_string()))
        );
    }

    #[test]
    fn parse_editor_from_pgrep_missing_fields_returns_none() {
        assert_eq!(parse_editor_from_pgrep(""), None);
        // No --port.
        assert_eq!(
            parse_editor_from_pgrep(
                "1 openvscode-server --connection-token c --server-base-path /s/t"
            ),
            None
        );
        // No --connection-token.
        assert_eq!(
            parse_editor_from_pgrep("1 openvscode-server --port 9100 --server-base-path /s/t"),
            None
        );
        // base-path not under /s/.
        assert_eq!(
            parse_editor_from_pgrep(
                "1 openvscode-server --port 9100 --connection-token c --server-base-path /x/t"
            ),
            None
        );
    }
}
