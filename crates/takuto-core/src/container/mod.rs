// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Docker container orchestration for Takuto workflows.
//!
//! This module is split into focused sub-modules following the
//! `workflow/engine/` pattern. The `mod.rs` facade re-exports the public
//! surface so existing callers using `takuto_core::container::*` /
//! `crate::container::*` paths continue to compile unchanged.
//!
//! Sub-modules:
//! - [`runner`]: [`ContainerRunner`] struct + impl plus shared `docker run`
//!   scaffolding (env vars, volume mounts, `WorkerSecretsBundle` plumbing).
//! - [`editor`]: openvscode-server editor container lifecycle and the URL /
//!   token helpers used by the shared-port reverse proxy.
//! - [`terminal`]: ttyd web terminal lifecycle inside a running editor
//!   container.
//! - [`reap`]: zombie container removal and dangling-image pruning.
//! - [`port_scanner`]: dynamic port forwarding via `socat`.
//! - [`run_command`]: per-step user-defined run-command containers and
//!   their dedicated port scanner.

pub(crate) mod dind_paths;
pub(crate) mod docker_args;
pub(crate) mod editor;
pub(crate) mod port_scanner;
pub(crate) mod reap;
pub(crate) mod run_command;
pub(crate) mod runner;
pub mod runtime;
pub(crate) mod secrets_bundle;
pub(crate) mod terminal;
pub(crate) mod volumes;
pub mod workspace;
pub(crate) mod wrap_command;

// ---------------------------------------------------------------------------
// Re-exports — preserve the pre-split public surface
// ---------------------------------------------------------------------------

pub use editor::{
    EditorInfo, allocate_single_port, build_editor_url, build_session_dynamic_port_url,
    build_session_editor_url, build_session_terminal_url, build_terminal_url,
    generate_connection_token, generate_session_path_token, get_editor_info,
    parse_connection_token_from_labels, parse_label_value, release_editor_ports,
    session_publish_arg, start_editor, stop_editor,
};
pub use port_scanner::{listening_ports_in_editor, run_port_scanner};
pub use run_command::{
    RunCommandInfo, is_run_command_running, run_command_container_name,
    run_run_command_port_scanner, start_run_command, stop_all_run_commands, stop_run_command,
};
pub use runner::ContainerRunner;
pub use runtime::{ContainerRuntime, DockerRuntime};
pub use terminal::{
    find_running_terminal, parse_terminal_auth_from_pgrep, start_terminal, stop_terminal,
};
pub use volumes::build_volume_args;

// `pub(crate)` re-export so internal callers that reach
// `crate::container::apply_secrets_bundle_to_args` (e.g. `auth/bundle.rs`
// tests) keep compiling unchanged after the split. Used only in
// `#[cfg(test)]` paths today; the `#[allow]` silences the unused-import
// warning in non-test builds without breaking the stable path.
#[allow(unused_imports)]
pub(crate) use secrets_bundle::apply_secrets_bundle_to_args;

// ---------------------------------------------------------------------------
// Shared helpers used by ≥ 2 sub-modules
// ---------------------------------------------------------------------------

/// Shell-escape a string for safe inclusion in `sh -c "..."`.
pub(crate) fn shell_escape(s: &str) -> String {
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
pub(crate) fn sanitize_ticket_key(key: &str) -> String {
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
pub(crate) fn is_dind_mode() -> bool {
    std::env::var("DOCKER_HOST").is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_ticket_key_lowercases_and_replaces() {
        assert_eq!(sanitize_ticket_key("PROJ-123"), "proj-123");
        assert_eq!(sanitize_ticket_key("My_Ticket.1"), "my-ticket-1");
    }
}
