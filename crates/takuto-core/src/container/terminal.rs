// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Web terminal (ttyd) lifecycle inside a running editor container.

use tracing::{debug, info};

use super::editor::{build_terminal_url, editor_container_name, generate_connection_token};
use super::editor_host_port;
use super::workspace::{WorkspaceStatus, workspace_status};

/// Start a web-based terminal (ttyd) inside the running editor container on `port`.
/// Returns the URL on success. Setup commands (tool installs, etc.) are expected to
/// have already been run at editor container creation by `run_editor_setup_as_root`.
pub async fn start_terminal(
    ticket_key: &str,
    port: u16,
) -> std::result::Result<(String, String), String> {
    let name = editor_container_name(ticket_key);

    // The shared workspace container must be running — the caller
    // (`open_terminal`) brings it up on demand. This checks the CONTAINER, not
    // the IDE: a terminal does not require openvscode-server to be running.
    if workspace_status(ticket_key).await != WorkspaceStatus::Running {
        return Err("Workspace container is not running.".into());
    }

    // Build the shell script that runs in each ttyd terminal:
    // 1. Source the takuto env file (Claude auth tokens, API keys, etc.)
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
    let shell_cmd = r#"[ -f /etc/takuto/env ] && set -a && . /etc/takuto/env && set +a
# Source the centralized GitHub App token so gh CLI and git operations authenticate.
if [ -f "$HOME/.config/gh/gh-app-token" ]; then
  # Don't clobber a per-user PAT the bundle already put in GH_TOKEN — only
  # fall back to the App-token file when GH_TOKEN is unset.
  export GH_TOKEN="${GH_TOKEN:-$(cat "$HOME/.config/gh/gh-app-token")}"
  # Configure git credential helper (editor containers don't inherit the main
  # container's ~/.gitconfig). GH_TOKEN-first so a PAT overrides the App token.
  git config --global credential.https://github.com.helper \
    '!f() { echo protocol=https; echo host=github.com; echo username=x-access-token; echo "password=${GH_TOKEN:-$(cat $HOME/.config/gh/gh-app-token 2>/dev/null)}"; }; f' 2>/dev/null
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
    // `pkill` is absent from the workspace image; kill via a `/proc` scan.
    crate::container::pkill_in_container(&name, "ttyd").await;
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
