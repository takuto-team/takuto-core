// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! URL builders for direct (`http://localhost:<port>/…`) and shared-port
//! proxy (`/s/<token>/…`) editor + terminal endpoints, plus the
//! `docker --publish` argument that gates whether ports bind to loopback.

use super::token_gen::HEX_DIGITS;

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
pub(super) fn encode_query_value(s: &str) -> String {
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
/// reachable only by the takuto-web reverse proxy, not by anyone on the LAN.
///
/// When running with DinD (`DOCKER_HOST` is set), publishes to all interfaces
/// (`HOST:CONTAINER`) so the proxy in the takuto container can reach the
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
