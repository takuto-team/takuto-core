// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Parse `/s/{token}/{rest}` paths, hash tokens for safe logging, and build
//! the empty 404 / 308 responses the proxy uses for missing or trailing-slash
//! redirects. Also: detect WebSocket-upgrade requests.

use axum::body::Body;
use axum::http::{Request, Response, StatusCode, header};
use hyper::header::{CONNECTION, UPGRADE};
use sha2::{Digest, Sha256};

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

/// Build a 404 with no body. The body must NOT contain the token, the
/// kind, or any other discoverable information.
pub(super) fn not_found() -> Response<Body> {
    // SAFETY: Response::builder() with only a `StatusCode` set + an empty
    // body cannot fail — no header validation, no body construction risk.
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Body::empty())
        .expect("status + empty body is infallible")
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
pub(super) fn redirect_to_trailing_slash(token: &str, query: Option<&str>) -> Response<Body> {
    let location = match query {
        Some(q) if !q.is_empty() => format!("/s/{token}/?{q}"),
        _ => format!("/s/{token}/"),
    };
    // SAFETY: `location` is constructed above by string-concatenating a
    // pre-validated path token with `/`; both segments are ASCII so the
    // `LOCATION` header value parse cannot fail.
    Response::builder()
        .status(StatusCode::PERMANENT_REDIRECT)
        .header(header::LOCATION, location)
        .body(Body::empty())
        .expect("LOCATION header is ASCII, body is empty")
}

/// `true` if the request is a WebSocket upgrade.
///
/// We require BOTH `Upgrade: websocket` AND `Connection: upgrade` (case
/// insensitive) per RFC 6455 §4.1. Anything else falls through to the
/// HTTP forwarding path.
pub(super) fn is_websocket_upgrade<B>(req: &Request<B>) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

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
