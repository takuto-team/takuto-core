// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! HTTP + WebSocket reverse-proxy that fronts every editor and terminal
//! session under a single dashboard port (GH-45).
//!
//! Path shape: `/s/{path-token}/{*rest}`
//!  * `path-token` is the 32-char hex token registered in the
//!    [`PathTokenRegistry`] when the session was opened.
//!  * `rest` is forwarded verbatim (with the original query string) to the
//!    backend listener at `127.0.0.1:<host_port>`.
//!
//! Behaviour required by the GH-45 acceptance criteria:
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

use crate::session_registry::SessionRoute;
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
fn http_client() -> &'static Client<HttpConnector, Body> {
    static CLIENT: OnceLock<Client<HttpConnector, Body>> = OnceLock::new();
    CLIENT.get_or_init(|| Client::builder(TokioExecutor::new()).build_http())
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
pub fn parse_session_path(path: &str) -> Option<(String, Option<String>)> {
    let after_prefix = path.strip_prefix("/s/")?;
    if after_prefix.is_empty() {
        return None;
    }
    match after_prefix.find('/') {
        None => Some((after_prefix.to_string(), None)),
        Some(idx) => {
            let token = &after_prefix[..idx];
            let rest = &after_prefix[idx..];
            if token.is_empty() {
                return None;
            }
            Some((token.to_string(), Some(rest.to_string())))
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
fn redirect_to_trailing_slash(token: &str) -> Response<Body> {
    let location = format!("/s/{token}/");
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

/// Rewrite an incoming request URI to point at the backend listener. The
/// `/s/{token}` prefix is stripped; the remainder of the path (`rest`) and
/// the original query string are preserved verbatim.
///
/// Returns `None` if the rewritten URI is unparseable (e.g. the caller
/// passed a malformed `rest` slice). The proxy converts this into a 404
/// rather than risk forwarding an attacker-controlled URI.
pub fn build_upstream_uri(host_port: u16, rest: &str, query: Option<&str>) -> Option<Uri> {
    let path = if rest.is_empty() { "/" } else { rest };
    let qmark_query = query
        .filter(|q| !q.is_empty())
        .map(|q| format!("?{q}"))
        .unwrap_or_default();
    let raw = format!("http://127.0.0.1:{host_port}{path}{qmark_query}");
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
    let host =
        HeaderValue::from_str(&format!("127.0.0.1:{host_port}")).expect("host header is ascii");
    req.headers_mut().insert(HOST, host);
}

/// Top-level handler registered at `/s/{*rest}`.
pub async fn proxy_session(State(state): State<AppState>, req: Request<Body>) -> Response<Body> {
    let path = req.uri().path().to_string();
    let (token, rest) = match parse_session_path(&path) {
        Some(parts) => parts,
        None => return not_found(),
    };

    let rest = match rest {
        None => return redirect_to_trailing_slash(&token),
        Some(r) => r,
    };

    let route = match state.path_token_registry.lookup(&token).await {
        Some(r) => r,
        None => {
            // Per the GH-45 description's defense-in-depth recommendations,
            // failed path-resolution attempts must be visible at production
            // log levels (anomaly / brute-force-enumeration detection).
            // Logged with the SHA-256 prefix only — never the raw token.
            tracing::warn!(
                token_hash = %token_hash_prefix(&token),
                "session path token not found"
            );
            return not_found();
        }
    };

    if is_websocket_upgrade(&req) {
        forward_websocket(req, route, &rest).await
    } else {
        forward_http(req, route, &rest).await
    }
}

/// Forward a plain HTTP request to the backend.
async fn forward_http(mut req: Request<Body>, route: SessionRoute, rest: &str) -> Response<Body> {
    let query = req.uri().query().map(|s| s.to_string());
    let upstream_uri = match build_upstream_uri(route.host_port, rest, query.as_deref()) {
        Some(uri) => uri,
        None => return not_found(),
    };

    sanitise_request_headers(&mut req, route.host_port, false);
    *req.uri_mut() = upstream_uri;

    match http_client().request(req).await {
        Ok(resp) => {
            // hyper-util returns Response<Incoming>; convert body to axum Body.
            let (parts, body) = resp.into_parts();
            let body = Body::new(body.map_err(axum::Error::new));
            Response::from_parts(parts, body)
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
) -> Response<Body> {
    let query = req.uri().query().map(|s| s.to_string());
    let upstream_uri = match build_upstream_uri(route.host_port, rest, query.as_deref()) {
        Some(uri) => uri,
        None => return not_found(),
    };

    // Take the inbound upgrade future BEFORE forwarding, so we can pair it
    // with the upstream's upgrade once the 101 returns.
    let inbound_upgrade = hyper::upgrade::on(&mut req);

    sanitise_request_headers(&mut req, route.host_port, true);
    *req.uri_mut() = upstream_uri;

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
        assert_eq!(r.as_deref(), Some("/"));
    }

    #[test]
    fn parse_session_path_token_with_rest() {
        let (t, r) = parse_session_path("/s/abcdef/foo/bar").unwrap();
        assert_eq!(t, "abcdef");
        assert_eq!(r.as_deref(), Some("/foo/bar"));
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
    fn build_upstream_uri_strips_prefix_and_targets_loopback() {
        let uri = build_upstream_uri(9101, "/foo", None).unwrap();
        assert_eq!(uri.scheme_str(), Some("http"));
        assert_eq!(uri.host(), Some("127.0.0.1"));
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
        let resp = redirect_to_trailing_slash("0123456789abcdef0123456789abcdef");
        assert_eq!(resp.status(), StatusCode::PERMANENT_REDIRECT);
        let loc = resp
            .headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(loc, "/s/0123456789abcdef0123456789abcdef/");
    }
}
