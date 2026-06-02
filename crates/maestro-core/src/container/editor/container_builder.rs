// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! `docker run` orchestration for openvscode-server containers, plus the
//! `EditorInfo` public type returned to the dashboard. Lifecycle:
//! [`start_editor`] → [`get_editor_info`] (idempotent reconnect) →
//! [`stop_editor`].

use std::path::Path;

use tracing::{info, warn};

use super::super::runner::{
    PASSTHROUGH_ENV, WORKER_ENV, apply_secrets_bundle_to_args, build_volume_args,
    passthrough_is_bundled,
};
use super::super::{editor_host_port, is_dind_mode, shell_escape};
use super::labels::{editor_container_name, parse_connection_token_from_labels, parse_label_value};
use super::port_alloc::{
    EDITOR_PORT_MAX, EDITOR_PORT_MIN, allocate_editor_ports, release_container_ports,
};
use super::token_gen::{generate_connection_token, generate_session_path_token};
use super::urls::{build_editor_url, session_publish_arg};

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
    /// a container label (`maestro.path_token`) so `get_editor_info` can
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
    // is bind-mounted at `/run/maestro-secrets:ro`. Token bytes reach the
    // editor's in-browser terminal via the file path, not `docker
    // inspect`.
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
            // The bundle owns this secret; suppress the ambient host
            // value so `docker inspect` cannot leak it.
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

    // Attach the bundle AFTER the standard volumes so the bundle's `-v`
    // and `-e` flags are colocated.
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
