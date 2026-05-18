// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! HTTP + WebSocket reverse-proxy that fronts every editor and terminal
//! session under a single dashboard port (GH-45).
//!
//! Path shape: `/s/{path-token}/{*rest}`
//!  * `path-token` is the 32-char hex token registered in the
//!    [`PathTokenRegistry`] when the session was opened.
//!  * `rest` is forwarded verbatim (with the original query string) to the
//!    upstream backend listener (derived from `DOCKER_HOST` in DinD mode,
//!    or `127.0.0.1` with local Docker).
//!
//! Behaviour required by the GH-45 acceptance criteria:
//!  * When a user database is configured, every request must carry a valid
//!    `maestro_session` cookie (database-backed session). Unauthenticated
//!    requests receive `401 Unauthorized` before the path token is even
//!    checked, so a leaked URL alone is not sufficient to access a session.
//!  * Unknown tokens → `404 Not Found`, empty body, no `kind` echoed back
//!    (anti-info-leak per AC #6).
//!  * `/s/{token}` with no trailing slash → `308 Permanent Redirect` to
//!    `/s/{token}/` so relative asset URLs from openvscode-server / ttyd
//!    resolve correctly.
//!  * WebSocket upgrade requests are token-validated **before** the 101
//!    handshake completes (AC #7) — an unknown token returns 404, never an
//!    upgrade response.
//!  * Successful WebSocket upgrades are tunnelled bidirectionally so the
//!    backend's existing `Upgrade: websocket` flow keeps working through
//!    the proxy.

use std::sync::OnceLock;

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderValue, Request, Response, StatusCode, Uri, header};
use axum::response::IntoResponse;
use http_body_util::BodyExt;
use hyper::header::{CONNECTION, HOST, UPGRADE};
use hyper_util::client::legacy::{Client, connect::HttpConnector};
use hyper_util::rt::TokioExecutor;
use sha2::{Digest, Sha256};

use crate::auth::{session_cookie_from_headers, validate_db_session};
use crate::session_registry::{SessionRoute, SessionRouteKind};
use crate::state::AppState;

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
fn http_client() -> &'static Client<HttpConnector, Body> {
    static CLIENT: OnceLock<Client<HttpConnector, Body>> = OnceLock::new();
    CLIENT.get_or_init(|| {
        let mut connector = HttpConnector::new();
        connector.set_connect_timeout(Some(std::time::Duration::from_secs(5)));
        Client::builder(TokioExecutor::new()).build(connector)
    })
}

/// Short, log-safe identifier for a path token: first 8 hex chars of
/// `SHA-256(token)`. We do this so failed-lookup logs (a likely target for
/// log-shipping) never echo back any byte of the actual token.
pub fn token_hash_prefix(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(8);
    for byte in &digest[..4] {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Parse `/s/<token>` (no trailing slash, no rest) or
/// `/s/<token>/<rest>` from a request path.
///
/// Returns `None` if the path does not start with `/s/<non-empty>`.
/// Returns `Some((token, None))` for the bare `/s/<token>` case (caller
/// must redirect to add a trailing slash).
/// Returns `Some((token, Some(rest)))` for `/s/<token>/<rest>` — `rest`
/// includes the leading slash, e.g. `/` for an empty rest, `/foo/bar` for
/// a deeper path.
///
/// Returns borrowed slices into `path` to avoid heap allocations on the
/// proxy hot path — VS Code editor sessions fire dozens of asset
/// sub-requests in quick succession.
pub fn parse_session_path(path: &str) -> Option<(&str, Option<&str>)> {
    let after_prefix = path.strip_prefix("/s/")?;
    if after_prefix.is_empty() {
        return None;
    }
    match after_prefix.find('/') {
        None => Some((after_prefix, None)),
        Some(idx) => {
            let token = &after_prefix[..idx];
            let rest = &after_prefix[idx..];
            if token.is_empty() {
                return None;
            }
            Some((token, Some(rest)))
        }
    }
}

/// Build a 404 with no body. Per GH-45 #6, the body must NOT contain the
/// token, the kind, or any other discoverable information.
fn not_found() -> Response<Body> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Body::empty())
        .expect("404 builder is infallible")
}

/// Build a 308 redirect to add a trailing slash to a `/s/{token}` URL.
///
/// 308 (Permanent Redirect) preserves the request method and body, which
/// is what we want — relative asset URLs from openvscode-server resolve
/// against the request URL, so the redirect must be terminal not silent.
///
/// The original query string (if any) is preserved so that manually copied
/// or bookmarked URLs like `/s/{token}?tkn=abc&folder=/x` don't lose their
/// auth parameters on redirect.
fn redirect_to_trailing_slash(token: &str, query: Option<&str>) -> Response<Body> {
    let location = match query {
        Some(q) if !q.is_empty() => format!("/s/{token}/?{q}"),
        _ => format!("/s/{token}/"),
    };
    Response::builder()
        .status(StatusCode::PERMANENT_REDIRECT)
        .header(header::LOCATION, location)
        .body(Body::empty())
        .expect("redirect builder is infallible")
}

/// `true` if the request is a WebSocket upgrade.
///
/// We require BOTH `Upgrade: websocket` AND `Connection: upgrade` (case
/// insensitive) per RFC 6455 §4.1. Anything else falls through to the
/// HTTP forwarding path.
fn is_websocket_upgrade<B>(req: &Request<B>) -> bool {
    let has_upgrade_header = req
        .headers()
        .get(UPGRADE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.eq_ignore_ascii_case("websocket"))
        .unwrap_or(false);
    let has_connection_upgrade = req
        .headers()
        .get(CONNECTION)
        .and_then(|v| v.to_str().ok())
        .map(|v| {
            v.split(',')
                .any(|part| part.trim().eq_ignore_ascii_case("upgrade"))
        })
        .unwrap_or(false);
    has_upgrade_header && has_connection_upgrade
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
fn upstream_host() -> &'static str {
    static HOST: OnceLock<String> = OnceLock::new();
    HOST.get_or_init(|| {
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
pub fn build_upstream_uri(host_port: u16, rest: &str, query: Option<&str>) -> Option<Uri> {
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
fn sanitise_request_headers(req: &mut Request<Body>, host_port: u16, is_upgrade: bool) {
    if !is_upgrade {
        for name in HOP_BY_HOP_HEADERS {
            req.headers_mut().remove(*name);
        }
    }
    let host = HeaderValue::from_str(&format!("{}:{host_port}", upstream_host()))
        .expect("host header is ascii");
    req.headers_mut().insert(HOST, host);
}

/// Fallback handler for root-relative asset requests from proxied apps.
///
/// Dev servers (Vite, Storybook, etc.) generate JS `import` statements with
/// root-relative paths (e.g. `import "/node_modules/.vite/deps/react.js"`).
/// The browser resolves these against the page origin, bypassing the
/// `/s/{token}/` proxy path. This handler catches those requests by checking
/// the `Referer` header: if it contains `/s/{token}/`, the request is proxied
/// to the same upstream.
///
/// Registered as the `fallback` handler, replacing the static file handler
/// for requests that match a known proxy session via referer.
pub async fn proxy_or_static_fallback(
    State(state): State<AppState>,
    req: Request<Body>,
) -> Response<Body> {
    // Unknown `/api/*` paths must NOT fall through to the SPA bundle — a route
    // that has been deleted (e.g. plan-10's removed `/api/workspaces`) should
    // return 404, not 200 with `index.html`. The SPA is for client-side routes
    // only; the dashboard's API surface is canonical.
    if req.uri().path().starts_with("/api/") {
        return (StatusCode::NOT_FOUND, "").into_response();
    }

    // Find a DynamicPort route via referer or cookie.
    let proxy_route = find_dynamic_port_route(&state, req.headers()).await;

    if let Some((token, route)) = proxy_route {
        let auth_user_id = authenticate_request(state.db.as_ref(), req.headers())
            .await
            .ok()
            .flatten();
        if user_owns_route(&auth_user_id, &route) {
            let path = req.uri().path().to_string();
            return forward_http(req, route, &path, &token).await;
        }
    }
    crate::server::serve_static(req.uri().clone()).await
}

/// Find a DynamicPort route for a root-relative asset request.
///
/// Strategy 1: `Referer` header contains `/s/{token}/` → use that token.
/// Strategy 2: `maestro_dynamic_port` cookie → set when the user last
///   visited a DynamicPort HTML page, catches deep JS import chains.
async fn find_dynamic_port_route(
    state: &AppState,
    headers: &axum::http::HeaderMap,
) -> Option<(String, SessionRoute)> {
    // Try referer first (most reliable — carries the exact token).
    if let Some(referer) = headers.get(header::REFERER).and_then(|v| v.to_str().ok())
        && let Some(token) = extract_token_from_referer(referer)
        && let Some(route) = state.path_token_registry.lookup(&token).await
        && route.kind == SessionRouteKind::DynamicPort
    {
        return Some((token, route));
    }
    // Fall back to cookie (covers deep dependency chains without referer).
    let token = extract_dynamic_port_cookie(headers)?;
    let route = state.path_token_registry.lookup(&token).await?;
    (route.kind == SessionRouteKind::DynamicPort).then_some((token, route))
}

/// Authenticate the request and return the user ID if valid.
///
/// Returns `Some(user_id)` when a valid `db-` session cookie is present.
/// Returns `None` when no database is configured (auth skipped).
/// Returns `Err(())` when auth is required but the cookie is missing/invalid.
async fn authenticate_request(
    db: Option<&maestro_core::db::Database>,
    headers: &axum::http::HeaderMap,
) -> Result<Option<String>, ()> {
    let Some(db) = db else {
        return Ok(None);
    };
    match session_cookie_from_headers(headers) {
        Some(raw) if raw.starts_with("db-") => {
            let db = db.clone();
            let cookie = raw.to_string();
            let uid = tokio::task::spawn_blocking(move || {
                let conn = db.conn().blocking_lock();
                validate_db_session(&conn, &cookie)
            })
            .await
            .unwrap_or(None);
            uid.map(Some).ok_or(())
        }
        _ => Err(()),
    }
}

/// Check that the authenticated user owns the given route.
fn user_owns_route(auth_user_id: &Option<String>, route: &SessionRoute) -> bool {
    match auth_user_id {
        Some(uid) => route.user_id == *uid,
        None => true, // no DB configured, skip ownership check
    }
}

/// Extract the `maestro_dynamic_port` cookie value (a proxy token).
fn extract_dynamic_port_cookie(headers: &axum::http::HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    for part in raw.split(';') {
        let part = part.trim();
        if let Some((name, value)) = part.split_once('=')
            && name.trim() == "maestro_dynamic_port"
        {
            let token = value.trim();
            if !token.is_empty() {
                return Some(token.to_string());
            }
        }
    }
    None
}

/// Extract a session token from a Referer URL like
/// `http://host:port/s/{token}/…`.
fn extract_token_from_referer(referer: &str) -> Option<String> {
    // Find "/s/" in the referer URL and extract the token segment.
    let after_s = referer.find("/s/").map(|i| &referer[i + 3..])?;
    let token = after_s.split('/').next()?;
    if token.is_empty() {
        return None;
    }
    Some(token.to_string())
}

/// Top-level handler registered at `/s/{*rest}`.
///
/// When a user database is configured (`state.db` is `Some`), the handler
/// requires a valid `maestro_session` cookie before proceeding. This prevents
/// access if the URL leaks — the unguessable path token remains a defence in
/// depth, but is no longer the sole gatekeeper.
pub async fn proxy_session(State(state): State<AppState>, req: Request<Body>) -> Response<Body> {
    let auth_user_id = match authenticate_request(state.db.as_ref(), req.headers()).await {
        Ok(uid) => uid,
        Err(()) => {
            return Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .body(Body::empty())
                .expect("401 builder is infallible");
        }
    };

    let path = req.uri().path().to_string();
    let query = req.uri().query().map(|s| s.to_string());
    let (token, rest) = match parse_session_path(&path) {
        Some(parts) => parts,
        None => return not_found(),
    };
    let rest = match rest {
        None => return redirect_to_trailing_slash(token, query.as_deref()),
        Some(r) => r,
    };
    let route = match state.path_token_registry.lookup(token).await {
        Some(r) => r,
        None => {
            tracing::warn!(
                token_hash = %token_hash_prefix(token),
                "session path token not found"
            );
            return not_found();
        }
    };
    if !user_owns_route(&auth_user_id, &route) {
        tracing::warn!(
            token_hash = %token_hash_prefix(token),
            "session route user_id mismatch"
        );
        return not_found();
    }

    if is_websocket_upgrade(&req) {
        forward_websocket(req, route, rest, token).await
    } else {
        forward_http(req, route, rest, token).await
    }
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
fn upstream_path_for_kind(kind: SessionRouteKind, token: &str, rest: &str) -> String {
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
async fn forward_http(
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
                && let Ok(cookie_val) = HeaderValue::from_str(
                    &format!("maestro_dynamic_port={token}; Path=/; SameSite=Lax")
                )
            {
                response.headers_mut().append(header::SET_COOKIE, cookie_val);
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

/// Forward a WebSocket upgrade. The token has already been validated by
/// `proxy_session` BEFORE entering this function — that ordering is what
/// satisfies AC #7 (the 101 response is never emitted on an unknown token).
async fn forward_websocket(
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

    // Take the inbound upgrade future BEFORE forwarding, so we can pair it
    // with the upstream's upgrade once the 101 returns.
    let inbound_upgrade = hyper::upgrade::on(&mut req);

    sanitise_request_headers(&mut req, route.host_port, true);
    *req.uri_mut() = upstream_uri.clone();

    tracing::debug!(
        kind = route.kind.as_str(),
        host_port = route.host_port,
        upstream = %upstream_uri,
        "session proxy forwarding websocket upgrade"
    );

    let mut upstream_resp = match http_client().request(req).await {
        Ok(r) => r,
        Err(err) => {
            tracing::warn!(
                kind = route.kind.as_str(),
                host_port = route.host_port,
                error = %err,
                "session proxy upstream upgrade request failed"
            );
            return (StatusCode::BAD_GATEWAY, "upstream unavailable").into_response();
        }
    };

    if upstream_resp.status() != StatusCode::SWITCHING_PROTOCOLS {
        // Backend refused the upgrade — pass its response through.
        let (parts, body) = upstream_resp.into_parts();
        let body = Body::new(body.map_err(axum::Error::new));
        return Response::from_parts(parts, body);
    }

    // Take the upstream upgrade future BEFORE moving the response into parts
    // so we extract it from the live response's extensions, not from a clone.
    // The 101 response we return below tells the client the upgrade succeeded;
    // once axum hands us the raw socket via `hyper::upgrade::on(&mut req)`,
    // we glue it to this upstream upgrade.
    let upstream_upgrade = hyper::upgrade::on(&mut upstream_resp);
    let (parts, _body) = upstream_resp.into_parts();
    tokio::spawn(async move {
        let (client_io, server_io) = match tokio::join!(inbound_upgrade, upstream_upgrade) {
            (Ok(c), Ok(s)) => (c, s),
            (Err(e), _) | (_, Err(e)) => {
                tracing::debug!(error = %e, "websocket upgrade pairing failed");
                return;
            }
        };
        let mut client_io = hyper_util::rt::TokioIo::new(client_io);
        let mut server_io = hyper_util::rt::TokioIo::new(server_io);
        if let Err(e) = tokio::io::copy_bidirectional(&mut client_io, &mut server_io).await {
            tracing::debug!(error = %e, "websocket tunnel ended with error");
        }
    });

    // Build the 101 response sent back to the client. We must replay the
    // upstream's response headers (Sec-WebSocket-Accept, Upgrade, Connection)
    // verbatim — those headers are what completes the RFC-6455 handshake.
    let mut response = Response::builder().status(StatusCode::SWITCHING_PROTOCOLS);
    if let Some(headers) = response.headers_mut() {
        for (name, value) in parts.headers.iter() {
            headers.insert(name.clone(), value.clone());
        }
    }
    response
        .body(Body::empty())
        .expect("101 response is well-formed")
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    // Integration tests — full handler through the router
    // -----------------------------------------------------------------

    mod integration {
        use std::collections::HashMap;
        use std::sync::Arc;
        use std::sync::atomic::AtomicBool;

        use axum::body::Body;
        use axum::http::{Request, StatusCode, header};
        use tokio::sync::RwLock;
        use tower::ServiceExt;

        use maestro_core::actions::dry_run::DryRunActions;
        use maestro_core::config::{Config, TicketingSystem};
        use maestro_core::workflow::engine::WorkflowEngine;

        use crate::server::build_router;
        use crate::session_registry::{PathTokenRegistry, SessionRoute, SessionRouteKind};
        use crate::state::AppState;

        fn test_state() -> AppState {
            let config = Arc::new(RwLock::new(Config::default()));
            let actions: Arc<dyn maestro_core::actions::traits::ExternalActions> = Arc::new(
                DryRunActions::new("origin".to_string(), None),
            );
            let jira_available = Arc::new(AtomicBool::new(false));
            let engine = Arc::new(WorkflowEngine::new(
                config.clone(),
                actions,
                1,
                jira_available.clone(),
                TicketingSystem::None,
                std::env::temp_dir(),
            ));
            AppState {
                engine,
                config,
                db: None,
                polling_paused: Arc::new(AtomicBool::new(false)),
                jira_available,
                ticketing_system: TicketingSystem::None,
                editor_scanners: Arc::new(RwLock::new(HashMap::new())),
                dynamic_forwards: Arc::new(RwLock::new(HashMap::new())),
                terminal_ports: Arc::new(RwLock::new(HashMap::new())),
                run_commands: Arc::new(RwLock::new(HashMap::new())),
                preflight_error: None,
                system_status: std::sync::Arc::new(tokio::sync::RwLock::new(maestro_core::docker_hooks::SystemStatus::default())),
                config_path: std::env::temp_dir().join("config.toml"),
                config_writer: None,
                clone_in_progress: Arc::new(AtomicBool::new(false)),
                path_token_registry: PathTokenRegistry::new(),
            }
        }

        /// Registered token with no upstream listener → 502 Bad Gateway.
        /// This proves the handler resolved the token, built the upstream URI,
        /// and attempted forwarding (connection refused ≈ 502).
        #[tokio::test]
        async fn proxy_known_token_no_upstream_returns_502() {
            let state = test_state();
            let token = state
                .path_token_registry
                .register_with_token(
                    "aaaa1111bbbb2222cccc3333dddd4444".to_string(),
                    SessionRoute {
                        kind: SessionRouteKind::Editor,
                        host_port: 19999, // nothing listens here
                        ticket_key: "TEST-1".to_string(),
                        user_id: "test-user".to_string(),
                    },
                )
                .await;
            assert!(token);

            let app = build_router(state);
            let resp = app
                .oneshot(
                    Request::get("/s/aaaa1111bbbb2222cccc3333dddd4444/")
                        .header("Host", "localhost")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
        }

        /// Unknown token → 404 with empty body (anti-info-leak).
        #[tokio::test]
        async fn proxy_unknown_token_returns_404() {
            let state = test_state();
            let app = build_router(state);
            let resp = app
                .oneshot(
                    Request::get("/s/deadbeefdeadbeefdeadbeefdeadbeef/foo")
                        .header("Host", "localhost")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(resp.status(), StatusCode::NOT_FOUND);
            let body = http_body_util::BodyExt::collect(resp.into_body())
                .await
                .unwrap()
                .to_bytes();
            assert_eq!(body.len(), 0, "404 body must be empty (no info leak)");
        }

        /// Bare `/s/{token}` (no trailing slash) → 308 redirect to `/s/{token}/`.
        #[tokio::test]
        async fn proxy_bare_token_redirects_with_trailing_slash() {
            let state = test_state();
            let token_str = "eeee5555ffff6666aaaa7777bbbb8888";
            state
                .path_token_registry
                .register_with_token(
                    token_str.to_string(),
                    SessionRoute {
                        kind: SessionRouteKind::Terminal,
                        host_port: 19998,
                        ticket_key: "TEST-2".to_string(),
                        user_id: "test-user".to_string(),
                    },
                )
                .await;

            let app = build_router(state);
            let resp = app
                .oneshot(
                    Request::get(format!("/s/{token_str}"))
                        .header("Host", "localhost")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(resp.status(), StatusCode::PERMANENT_REDIRECT);
            let loc = resp
                .headers()
                .get(header::LOCATION)
                .unwrap()
                .to_str()
                .unwrap();
            assert_eq!(loc, format!("/s/{token_str}/"));
        }

        // -----------------------------------------------------------------
        // Cookie auth gate (db-backed sessions)
        // -----------------------------------------------------------------

        /// When a database is present, requests without a session cookie must
        /// be rejected with 401 — even if the path token is valid.
        #[tokio::test]
        async fn proxy_returns_401_when_db_present_but_no_cookie() {
            let state = crate::test_helpers::test_state_with_db();
            state
                .path_token_registry
                .register_with_token(
                    "aaaa1111bbbb2222cccc3333dddd4444".to_string(),
                    SessionRoute {
                        kind: SessionRouteKind::Editor,
                        host_port: 19999,
                        ticket_key: "TEST-1".to_string(),
                        user_id: "test-user".to_string(),
                    },
                )
                .await;

            let app = build_router(state);
            let resp = app
                .oneshot(
                    Request::get("/s/aaaa1111bbbb2222cccc3333dddd4444/")
                        .header("Host", "localhost")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        }

        /// An invalid session cookie must also be rejected with 401.
        #[tokio::test]
        async fn proxy_returns_401_when_db_present_with_invalid_cookie() {
            let state = crate::test_helpers::test_state_with_db();
            state
                .path_token_registry
                .register_with_token(
                    "aaaa1111bbbb2222cccc3333dddd4444".to_string(),
                    SessionRoute {
                        kind: SessionRouteKind::Editor,
                        host_port: 19999,
                        ticket_key: "TEST-1".to_string(),
                        user_id: "test-user".to_string(),
                    },
                )
                .await;

            let app = build_router(state);
            let resp = app
                .oneshot(
                    Request::get("/s/aaaa1111bbbb2222cccc3333dddd4444/")
                        .header("Host", "localhost")
                        .header("Cookie", "maestro_session=db-nonexistent-session-id")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        }

        /// A valid session cookie must pass through to the proxy logic.
        /// Since there's no upstream listener, we expect 502 (Bad Gateway) —
        /// proving we got past the auth gate AND ownership check.
        #[tokio::test]
        async fn proxy_allows_with_valid_cookie() {
            let state = crate::test_helpers::test_state_with_db();
            let cookie = crate::test_helpers::register_and_login(&state).await;
            // Look up the admin user's ID so the route ownership check passes.
            let admin_user_id = {
                let db = state.db.as_ref().unwrap();
                let conn = db.conn().lock().await;
                maestro_core::db::users::get_user_by_username(&conn, "admin")
                    .unwrap()
                    .unwrap()
                    .id
            };
            state
                .path_token_registry
                .register_with_token(
                    "aaaa1111bbbb2222cccc3333dddd4444".to_string(),
                    SessionRoute {
                        kind: SessionRouteKind::Editor,
                        host_port: 19999,
                        ticket_key: "TEST-1".to_string(),
                        user_id: admin_user_id,
                    },
                )
                .await;

            let app = build_router(state);
            let resp = app
                .oneshot(
                    Request::get("/s/aaaa1111bbbb2222cccc3333dddd4444/")
                        .header("Host", "localhost")
                        .header("Cookie", &cookie)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            // 502 = upstream unreachable, which means auth + ownership passed.
            assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
        }

        /// A valid session cookie for user A must NOT grant access to user B's
        /// session route — the proxy returns 404 (same as unknown token).
        #[tokio::test]
        async fn proxy_returns_404_when_user_mismatch() {
            let state = crate::test_helpers::test_state_with_db();
            let cookie = crate::test_helpers::register_and_login(&state).await;
            // Register a route owned by a different user.
            state
                .path_token_registry
                .register_with_token(
                    "aaaa1111bbbb2222cccc3333dddd4444".to_string(),
                    SessionRoute {
                        kind: SessionRouteKind::Editor,
                        host_port: 19999,
                        ticket_key: "TEST-1".to_string(),
                        user_id: "other-user-id".to_string(),
                    },
                )
                .await;

            let app = build_router(state);
            let resp = app
                .oneshot(
                    Request::get("/s/aaaa1111bbbb2222cccc3333dddd4444/")
                        .header("Host", "localhost")
                        .header("Cookie", &cookie)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        }
    }

    // -----------------------------------------------------------------
    // Path parser
    // -----------------------------------------------------------------

    #[test]
    fn parse_session_path_bare_token_returns_none_rest() {
        let (t, r) = parse_session_path("/s/abcdef").unwrap();
        assert_eq!(t, "abcdef");
        assert!(r.is_none());
    }

    #[test]
    fn parse_session_path_token_with_trailing_slash_returns_slash_rest() {
        let (t, r) = parse_session_path("/s/abcdef/").unwrap();
        assert_eq!(t, "abcdef");
        assert_eq!(r, Some("/"));
    }

    #[test]
    fn parse_session_path_token_with_rest() {
        let (t, r) = parse_session_path("/s/abcdef/foo/bar").unwrap();
        assert_eq!(t, "abcdef");
        assert_eq!(r, Some("/foo/bar"));
    }

    #[test]
    fn parse_session_path_rejects_non_session_path() {
        assert!(parse_session_path("/api/foo").is_none());
        assert!(parse_session_path("/").is_none());
        assert!(parse_session_path("").is_none());
    }

    #[test]
    fn parse_session_path_rejects_empty_token() {
        assert!(parse_session_path("/s/").is_none());
        assert!(parse_session_path("/s//rest").is_none());
    }

    // -----------------------------------------------------------------
    // Upstream URI builder
    // -----------------------------------------------------------------

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

    // -----------------------------------------------------------------
    // Token-hash log helper (anti-info-leak)
    // -----------------------------------------------------------------

    #[test]
    fn token_hash_prefix_is_8_chars_hex() {
        let h = token_hash_prefix("0123456789abcdef0123456789abcdef");
        assert_eq!(h.len(), 8);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn token_hash_prefix_does_not_contain_token_bytes() {
        // The whole point of hashing: the log must not echo back any prefix
        // of the original token.
        let token = "0123456789abcdef0123456789abcdef";
        let h = token_hash_prefix(token);
        assert!(!token.starts_with(&h), "hash leaks token prefix");
        assert!(!token.contains(&h), "hash byte-substring overlaps token");
    }

    #[test]
    fn token_hash_prefix_is_deterministic() {
        assert_eq!(token_hash_prefix("a"), token_hash_prefix("a"));
        assert_ne!(token_hash_prefix("a"), token_hash_prefix("b"));
    }

    // -----------------------------------------------------------------
    // WebSocket upgrade detection
    // -----------------------------------------------------------------

    fn req_with(headers: &[(&'static str, &'static str)]) -> Request<Body> {
        let mut b = Request::builder().uri("/s/x/").method("GET");
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        b.body(Body::empty()).unwrap()
    }

    #[test]
    fn is_websocket_upgrade_true_for_canonical_handshake() {
        let r = req_with(&[("upgrade", "websocket"), ("connection", "Upgrade")]);
        assert!(is_websocket_upgrade(&r));
    }

    #[test]
    fn is_websocket_upgrade_handles_mixed_case_and_connection_list() {
        let r = req_with(&[
            ("Upgrade", "WebSocket"),
            ("Connection", "keep-alive, Upgrade"),
        ]);
        assert!(is_websocket_upgrade(&r));
    }

    #[test]
    fn is_websocket_upgrade_false_for_plain_get() {
        let r = req_with(&[]);
        assert!(!is_websocket_upgrade(&r));
    }

    #[test]
    fn is_websocket_upgrade_false_when_only_upgrade_header() {
        let r = req_with(&[("upgrade", "websocket")]);
        assert!(!is_websocket_upgrade(&r));
    }

    #[test]
    fn is_websocket_upgrade_false_when_only_connection_upgrade() {
        let r = req_with(&[("connection", "upgrade")]);
        assert!(!is_websocket_upgrade(&r));
    }

    #[test]
    fn is_websocket_upgrade_false_for_non_ws_upgrade() {
        let r = req_with(&[("upgrade", "h2c"), ("connection", "Upgrade")]);
        assert!(!is_websocket_upgrade(&r));
    }

    // -----------------------------------------------------------------
    // 404 and redirect builders
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn not_found_response_has_empty_body_and_no_token_echo() {
        let resp = not_found();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = http_body_util::BodyExt::collect(resp.into_body())
            .await
            .unwrap()
            .to_bytes();
        assert_eq!(body.len(), 0, "404 body must be empty (no info leak)");
    }

    #[test]
    fn redirect_response_targets_trailing_slash_path() {
        let resp = redirect_to_trailing_slash("0123456789abcdef0123456789abcdef", None);
        assert_eq!(resp.status(), StatusCode::PERMANENT_REDIRECT);
        let loc = resp
            .headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(loc, "/s/0123456789abcdef0123456789abcdef/");
    }

    #[test]
    fn redirect_preserves_query_string() {
        let resp = redirect_to_trailing_slash(
            "0123456789abcdef0123456789abcdef",
            Some("tkn=abc&folder=/workspace/proj"),
        );
        assert_eq!(resp.status(), StatusCode::PERMANENT_REDIRECT);
        let loc = resp
            .headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(
            loc,
            "/s/0123456789abcdef0123456789abcdef/?tkn=abc&folder=/workspace/proj"
        );
    }

    #[test]
    fn redirect_with_empty_query_omits_question_mark() {
        let resp = redirect_to_trailing_slash("0123456789abcdef0123456789abcdef", Some(""));
        let loc = resp
            .headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(loc, "/s/0123456789abcdef0123456789abcdef/");
    }
}
