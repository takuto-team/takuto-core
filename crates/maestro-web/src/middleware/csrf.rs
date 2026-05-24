// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! CSRF protection via `Origin` / `Referer` allowlist (plan-02 AC-1).
//!
//! Every state-changing request (`POST`/`PUT`/`DELETE`/`PATCH`) on the dashboard
//! API must carry an `Origin` header that matches the configured CORS allowlist
//! (`[web] cors_origins`, or the auto-computed default). If `Origin` is absent
//! the middleware falls back to extracting the origin from `Referer`. Requests
//! whose origin is missing or does not match are rejected with **403 Forbidden**
//! before any handler (and before the auth middleware on `api_protected`) is
//! invoked.
//!
//! Safe methods (`GET`/`HEAD`/`OPTIONS`) are exempt: they cannot mutate state
//! and forcing browsers to send `Origin` on every fetch would interact poorly
//! with the static asset fallback.
//!
//! `/s/*` reverse-proxy paths are also exempt because they authenticate via the
//! opaque session token in the path, not via the dashboard cookie.

use axum::Json;
use axum::extract::{Request, State};
use axum::http::{Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::state::ConfigState;

/// Axum middleware enforcing the Origin/Referer allowlist on mutating requests.
///
/// Wire via `axum::middleware::from_fn_with_state(state, csrf::csrf_middleware)`.
pub async fn csrf_middleware(
    State(cfg): State<ConfigState>,
    request: Request,
    next: Next,
) -> Response {
    // Safe methods can never mutate state; let them through unconditionally.
    match *request.method() {
        Method::GET | Method::HEAD | Method::OPTIONS => return next.run(request).await,
        _ => {}
    }

    // The shared-port reverse proxy uses opaque path tokens; its own auth is the
    // gate. Skip CSRF entirely so editor/terminal HTTP traffic isn't broken.
    if request.uri().path().starts_with("/s/") {
        return next.run(request).await;
    }

    // Capture the method+path for structured logging on reject; cheap clones.
    let method = request.method().clone();
    let path = request.uri().path().to_string();

    // Read the canonical CORS allowlist. `resolved_cors_origins()` returns the
    // explicit list when configured, or an auto-computed default derived from
    // `host`/`port` when empty.
    let allowed_origins: Vec<String> = {
        let config = cfg.config.read().await;
        config.web.resolved_cors_origins()
    };

    // Prefer the `Origin` header (browsers attach it to fetch/XHR requests).
    // Fall back to `Referer` when `Origin` is absent (some older clients, or
    // form submissions from same-origin pages without explicit fetch metadata).
    let headers = request.headers();
    let origin_str: Option<String> = headers
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .or_else(|| {
            headers
                .get(header::REFERER)
                .and_then(|v| v.to_str().ok())
                .and_then(origin_from_referer)
        });

    let Some(origin) = origin_str else {
        tracing::warn!(
            event = "csrf_reject",
            method = %method,
            path = %path,
            reason = "missing_origin",
            "CSRF: rejecting mutating request with no Origin/Referer"
        );
        return reject();
    };

    if origin_in_allowlist(&origin, &allowed_origins) {
        next.run(request).await
    } else {
        tracing::warn!(
            event = "csrf_reject",
            method = %method,
            path = %path,
            origin = %origin,
            reason = "origin_mismatch",
            "CSRF: rejecting cross-origin mutating request"
        );
        reject()
    }
}

/// Build a 403 response with a small JSON body describing the failure.
fn reject() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({"error": "missing or invalid Origin/Referer"})),
    )
        .into_response()
}

/// Case-sensitive comparison of an `Origin` value against the configured
/// allowlist. Trailing slashes are tolerated on either side: `cors_origins`
/// validation forbids them on input, but browsers occasionally send `Origin`
/// values without trailing slashes regardless.
fn origin_in_allowlist(origin: &str, allowed: &[String]) -> bool {
    let needle = origin.trim_end_matches('/');
    allowed
        .iter()
        .any(|a| a.trim_end_matches('/').eq_ignore_ascii_case(needle))
}

/// Parse a `Referer` URL and return its origin (`scheme://authority`) or
/// `None` if the input does not start with `http://` / `https://` or has no
/// host.
///
/// We intentionally do NOT use a full URL parser — the `Referer` header is
/// always a fully-qualified URL per RFC 7231, and we only need the prefix up
/// to the path/query separator.
pub fn origin_from_referer(referer: &str) -> Option<String> {
    let referer = referer.trim();
    let (scheme, rest) = if let Some(r) = referer.strip_prefix("https://") {
        ("https", r)
    } else if let Some(r) = referer.strip_prefix("http://") {
        ("http", r)
    } else {
        return None;
    };
    // Authority ends at the first `/`, `?`, or `#`.
    let end = rest
        .find(['/', '?', '#'])
        .unwrap_or(rest.len());
    let authority = &rest[..end];
    if authority.is_empty() {
        return None;
    }
    Some(format!("{scheme}://{authority}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_from_referer_extracts_basic_http() {
        assert_eq!(
            origin_from_referer("http://localhost:8080/dashboard/workflows"),
            Some("http://localhost:8080".to_string())
        );
    }

    #[test]
    fn origin_from_referer_extracts_https_with_query() {
        assert_eq!(
            origin_from_referer("https://example.com/path?x=1"),
            Some("https://example.com".to_string())
        );
    }

    #[test]
    fn origin_from_referer_extracts_https_with_fragment() {
        assert_eq!(
            origin_from_referer("https://example.com#top"),
            Some("https://example.com".to_string())
        );
    }

    #[test]
    fn origin_from_referer_bare_host() {
        assert_eq!(
            origin_from_referer("http://h"),
            Some("http://h".to_string())
        );
    }

    #[test]
    fn origin_from_referer_rejects_missing_scheme() {
        assert_eq!(origin_from_referer("example.com/x"), None);
    }

    #[test]
    fn origin_from_referer_rejects_unsupported_scheme() {
        assert_eq!(origin_from_referer("ftp://example.com/x"), None);
    }

    #[test]
    fn origin_from_referer_rejects_empty_authority() {
        assert_eq!(origin_from_referer("http:///path"), None);
    }

    #[test]
    fn origin_in_allowlist_exact_match() {
        let allowed = vec!["http://localhost:8080".to_string()];
        assert!(origin_in_allowlist("http://localhost:8080", &allowed));
    }

    #[test]
    fn origin_in_allowlist_rejects_unknown() {
        let allowed = vec!["http://localhost:8080".to_string()];
        assert!(!origin_in_allowlist("https://evil.example", &allowed));
    }

    #[test]
    fn origin_in_allowlist_tolerates_trailing_slash_on_either_side() {
        let allowed = vec!["http://localhost:8080".to_string()];
        assert!(origin_in_allowlist("http://localhost:8080/", &allowed));
        let allowed = vec!["http://localhost:8080/".to_string()];
        assert!(origin_in_allowlist("http://localhost:8080", &allowed));
    }
}
