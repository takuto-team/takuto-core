// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Plain HTTP forwarding to the upstream session listener — request URI
//! rewriting, hop-by-hop header stripping, redirect rewriting, and the
//! hyper client itself.

use std::sync::OnceLock;

use axum::body::Body;
use axum::http::{HeaderValue, Request, Response, StatusCode, Uri, header};
use axum::response::IntoResponse;
use http_body_util::BodyExt;
use hyper::header::HOST;
use hyper_util::client::legacy::{Client, connect::HttpConnector};
use hyper_util::rt::TokioExecutor;

use crate::session_registry::{SessionRoute, SessionRouteKind};

use super::token_validator::not_found;

/// Hop-by-hop headers (RFC 7230 §6.1) that must NOT be forwarded.
///
/// `connection`, `upgrade`, `keep-alive`, `proxy-*`, `te`, `trailer`, and
/// `transfer-encoding` are scoped to the inbound hop. `Connection` and
/// `Upgrade` are special-cased separately for the WebSocket tunnel.
///
/// Stored as `&'static str` rather than `HeaderName` because `HeaderName`
/// has interior mutability (atomic refcount on `Bytes`), which makes it
/// invalid as the element type of a `const` slice. Conversion is cheap
/// (`HeaderName::from_static` is just a length check at debug time).
const HOP_BY_HOP_HEADERS: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// Process-wide hyper client. Lazily initialised so unit tests that never
/// hit the network can run without spinning up the executor.
///
/// A 5-second connect timeout prevents indefinite hangs when the upstream
/// host is unreachable (e.g. misconfigured DinD networking).
pub(super) fn http_client() -> &'static Client<HttpConnector, Body> {
    static CLIENT: OnceLock<Client<HttpConnector, Body>> = OnceLock::new();
    CLIENT.get_or_init(|| {
        let mut connector = HttpConnector::new();
        connector.set_connect_timeout(Some(std::time::Duration::from_secs(5)));
        Client::builder(TokioExecutor::new()).build(connector)
    })
}

/// The host the reverse proxy connects to for upstream session requests.
///
/// When `DOCKER_HOST` is set (DinD mode), the hostname is extracted from
/// the env var (e.g. `tcp://127.0.0.1:2375` → `127.0.0.1`).  In DinD
/// mode the maestro container shares the DinD container's network
/// namespace (`network_mode: service:dind`), so `127.0.0.1` reaches
/// docker-proxy ports directly — no cross-container routing required.
///
/// When `DOCKER_HOST` is unset (local Docker), the upstream is loopback.
/// Computed once and cached for the process lifetime.
pub(super) fn upstream_host() -> &'static str {
    static HOST_CACHE: OnceLock<String> = OnceLock::new();
    HOST_CACHE.get_or_init(|| {
        std::env::var("DOCKER_HOST")
            .ok()
            .and_then(|dh| {
                dh.strip_prefix("tcp://")
                    .and_then(|rest| rest.rsplit_once(':'))
                    .map(|(host, _port)| host.to_string())
            })
            .unwrap_or_else(|| "127.0.0.1".to_string())
    })
}

/// Rewrite an incoming request URI to point at the backend listener. The
/// `/s/{token}` prefix is stripped; the remainder of the path (`rest`) and
/// the original query string are preserved verbatim.
///
/// The upstream host is derived from `DOCKER_HOST` (DinD) or defaults to
/// `127.0.0.1` (local Docker). Returns `None` if the rewritten URI is
/// unparseable; the proxy converts this into a 404 rather than risk
/// forwarding an attacker-controlled URI.
pub(super) fn build_upstream_uri(host_port: u16, rest: &str, query: Option<&str>) -> Option<Uri> {
    let host = upstream_host();
    let path = if rest.is_empty() { "/" } else { rest };
    let qmark_query = query
        .filter(|q| !q.is_empty())
        .map(|q| format!("?{q}"))
        .unwrap_or_default();
    let raw = format!("http://{host}:{host_port}{path}{qmark_query}");
    raw.parse::<Uri>().ok()
}

/// Strip hop-by-hop headers and rewrite `Host` to point at the backend.
/// Used for plain HTTP forwarding only — the WS-upgrade path keeps
/// `Connection`/`Upgrade` intact intentionally (they ARE the upgrade).
pub(super) fn sanitise_request_headers(req: &mut Request<Body>, host_port: u16, is_upgrade: bool) {
    if !is_upgrade {
        for name in HOP_BY_HOP_HEADERS {
            req.headers_mut().remove(*name);
        }
    }
    // SAFETY: `upstream_host()` returns a literal `127.0.0.1` and `host_port`
    // is a `u16` rendered as decimal — both ASCII, both valid HTTP token chars.
    let host = HeaderValue::from_str(&format!("{}:{host_port}", upstream_host()))
        .expect("upstream_host() is ASCII, port is decimal u16");
    req.headers_mut().insert(HOST, host);
}

/// Rewrite root-relative `Location` headers in 3xx responses so the
/// browser stays within the `/s/{token}/` proxy namespace.
///
/// Without this, upstream backends (openvscode-server, ttyd) that
/// redirect to `/foo` would send the browser to `http://host:8080/foo`
/// — bypassing the proxy entirely. By prepending `/s/{token}`, the
/// redirect keeps going through the reverse proxy.
fn rewrite_redirect_location(resp: &mut Response<Body>, token: &str) {
    if !resp.status().is_redirection() {
        return;
    }
    let location = match resp.headers().get(header::LOCATION) {
        Some(v) => v.clone(),
        None => return,
    };
    let location_str = match location.to_str() {
        Ok(s) => s,
        Err(_) => return,
    };
    // Only rewrite root-relative paths (starting with `/`).
    // Absolute URLs and relative paths are left untouched.
    // Skip if the location already contains /s/{token} (app knows its base).
    if let Some(path) = location_str.strip_prefix('/') {
        if path.starts_with(&format!("s/{token}")) {
            return;
        }
        let rewritten = format!("/s/{token}/{path}");
        if let Ok(val) = HeaderValue::from_str(&rewritten) {
            resp.headers_mut().insert(header::LOCATION, val);
        }
    }
}

/// Compute the upstream path based on session kind.
///
/// Editor sessions use `--server-base-path /s/{token}` so the full prefix
/// must be preserved. Terminal sessions strip the proxy prefix.
pub(super) fn upstream_path_for_kind(kind: SessionRouteKind, token: &str, rest: &str) -> String {
    match kind {
        SessionRouteKind::Editor => format!("/s/{token}{rest}"),
        // Terminal & DynamicPort: strip the proxy prefix so the upstream
        // receives requests at root. Apps that need base-path awareness
        // (e.g. Vite) can use MAESTRO_PROXY_BASE; apps that don't (e.g.
        // Storybook, static servers) work transparently.
        SessionRouteKind::Terminal | SessionRouteKind::DynamicPort => rest.to_string(),
    }
}

/// Forward a plain HTTP request to the backend.
///
/// For **editor** sessions the upstream openvscode-server is launched with
/// `--server-base-path /s/{token}`, so the full `/s/{token}/…` prefix must be
/// kept in the forwarded path. For **terminal** (ttyd) sessions the prefix is
/// stripped — ttyd has no base-path equivalent.
pub(super) async fn forward_http(
    mut req: Request<Body>,
    route: SessionRoute,
    rest: &str,
    token: &str,
) -> Response<Body> {
    let query = req.uri().query().map(|s| s.to_string());
    let upstream_path = upstream_path_for_kind(route.kind, token, rest);
    let upstream_uri = match build_upstream_uri(route.host_port, &upstream_path, query.as_deref()) {
        Some(uri) => uri,
        None => return not_found(),
    };

    sanitise_request_headers(&mut req, route.host_port, false);
    *req.uri_mut() = upstream_uri.clone();

    tracing::debug!(
        kind = route.kind.as_str(),
        host_port = route.host_port,
        upstream = %upstream_uri,
        "session proxy forwarding request"
    );

    match http_client().request(req).await {
        Ok(resp) => {
            // hyper-util returns Response<Incoming>; convert body to axum Body.
            let (parts, body) = resp.into_parts();
            let body = Body::new(body.map_err(axum::Error::new));
            let mut response = Response::from_parts(parts, body);
            // Editor sessions with --server-base-path already emit correct
            // `/s/{token}/…` redirects — only terminal sessions need rewriting.
            if route.kind == SessionRouteKind::Terminal {
                rewrite_redirect_location(&mut response, token);
            }
            // For DynamicPort: set a cookie so the referer-based fallback in
            // proxy_or_static_fallback can identify the correct upstream for
            // root-relative asset requests (JS imports that bypass /s/{token}/).
            if route.kind == SessionRouteKind::DynamicPort
                && response
                    .headers()
                    .get(header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|ct| ct.starts_with("text/html"))
                && let Ok(cookie_val) = HeaderValue::from_str(&format!(
                    "maestro_dynamic_port={token}; Path=/; SameSite=Lax"
                ))
            {
                response
                    .headers_mut()
                    .append(header::SET_COOKIE, cookie_val);
            }
            response
        }
        Err(err) => {
            tracing::warn!(
                kind = route.kind.as_str(),
                host_port = route.host_port,
                error = %err,
                "session proxy upstream request failed"
            );
            (StatusCode::BAD_GATEWAY, "upstream unavailable").into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_upstream_uri_strips_prefix_and_targets_upstream_host() {
        let uri = build_upstream_uri(9101, "/foo", None).unwrap();
        assert_eq!(uri.scheme_str(), Some("http"));
        // Use upstream_host() rather than hardcoding 127.0.0.1 — CI
        // environments that set DOCKER_HOST would cause a false failure
        // (the OnceLock caches the first caller's result for the process).
        assert_eq!(uri.host(), Some(upstream_host()));
        assert_eq!(uri.port_u16(), Some(9101));
        assert_eq!(uri.path(), "/foo");
        assert_eq!(uri.query(), None);
    }

    #[test]
    fn build_upstream_uri_preserves_query_string() {
        let uri = build_upstream_uri(9101, "/", Some("tkn=abc&folder=/x")).unwrap();
        assert_eq!(uri.path(), "/");
        assert_eq!(uri.query(), Some("tkn=abc&folder=/x"));
    }

    #[test]
    fn build_upstream_uri_empty_rest_becomes_root() {
        let uri = build_upstream_uri(9101, "", None).unwrap();
        assert_eq!(uri.path(), "/");
    }
}
