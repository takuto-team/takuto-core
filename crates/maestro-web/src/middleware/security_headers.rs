// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Defence-in-depth security response headers.
//!
//! Sets the following headers on every response emitted by the top-level
//! router:
//!
//! - `Content-Security-Policy` — locks down origins for scripts, styles,
//!   connections, frames, base, and form submissions. **Not** sent on
//!   `/s/*` proxy responses because the editor (Coder/code-server) loads
//!   inline scripts.
//! - `X-Frame-Options: SAMEORIGIN` — defence against clickjacking even on
//!   browsers that haven't implemented `frame-ancestors`. Not sent on
//!   `/s/*` (the embedded editor needs a different framing policy and the
//!   header would conflict).
//! - `Referrer-Policy` — `strict-origin` for the dashboard, **overridden** to
//!   `no-referrer` on `/s/*` because the session path token is sensitive and
//!   must not leak via `Referer` to external links.
//! - `X-Content-Type-Options: nosniff` — applied on every response,
//!   including `/s/*`.
//! - `Strict-Transport-Security: max-age=31536000; includeSubDomains` —
//!   emitted only when the deployment is HTTPS (auto-detected; see
//!   [`resolve_https_context`]).
//!
//! Wire this as the **outermost** layer on the top-level router so the
//! headers are present even on error responses (401 from the auth middleware,
//! 403 from the CSRF middleware, 404 from the static fallback, etc.).

use axum::extract::{Request, State};
use axum::http::HeaderValue;
use axum::middleware::Next;
use axum::response::Response;

use crate::state::ConfigState;

/// Content-Security-Policy applied to dashboard responses. Tight defaults that
/// permit the React bundle (which uses `wasm-unsafe-eval` for some
/// CodeMirror/treesitter integrations) plus `unsafe-inline` styles (Tailwind
/// dev mode emits inline `<style>` tags during development).
pub const CSP: &str = "default-src 'self'; img-src 'self' data: blob:; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; connect-src 'self' wss:; frame-ancestors 'self'; base-uri 'self'; form-action 'self'";

const HSTS_VALUE: &str = "max-age=31536000; includeSubDomains";

/// Axum middleware that injects defence-in-depth response headers.
///
/// Captures the request path before consuming the request so the response-side
/// branch can know whether this was a `/s/*` proxy response (which needs a
/// looser header policy).
pub async fn security_headers_middleware(
    State(cfg): State<ConfigState>,
    request: Request,
    next: Next,
) -> Response {
    // Capture data we need from the request before `next.run` consumes it.
    // Pull HTTPS-context signals into owned values up-front so the
    // `resolve_https_context` future does not borrow `request` across an
    // await point (would make the future non-`Send`).
    let is_proxy_path = request.uri().path().starts_with("/s/");
    let xfp_is_https = request
        .headers()
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.eq_ignore_ascii_case("https"))
        .unwrap_or(false);
    let https_context = resolve_https_context(&cfg, xfp_is_https).await;

    let mut response = next.run(request).await;
    apply_headers(response.headers_mut(), is_proxy_path, https_context);
    response
}

/// Insert the security headers onto a response. Extracted so this can be
/// unit-tested without spinning up a full router.
fn apply_headers(
    headers: &mut axum::http::HeaderMap,
    is_proxy_path: bool,
    https_context: bool,
) {
    headers.insert("x-content-type-options", HeaderValue::from_static("nosniff"));

    if is_proxy_path {
        // Proxy responses: tighter `Referrer-Policy` (the path token is
        // sensitive), no CSP, no X-Frame-Options (the editor frames itself).
        headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    } else {
        headers.insert("content-security-policy", HeaderValue::from_static(CSP));
        headers.insert("x-frame-options", HeaderValue::from_static("SAMEORIGIN"));
        headers.insert("referrer-policy", HeaderValue::from_static("strict-origin"));
    }

    if https_context {
        headers.insert(
            "strict-transport-security",
            HeaderValue::from_static(HSTS_VALUE),
        );
    }
}

/// Detect whether the current request is being served in an HTTPS context.
///
/// Resolution order (highest precedence first):
/// 1. `X-Forwarded-Proto: https` on the inbound request (TLS-terminating proxy).
/// 2. Any `https://` entry in the resolved `cors_origins` allowlist.
///
/// `xfp_is_https` is pulled out by the caller so this future doesn't need to
/// borrow the `Request` across an await point.
///
/// The canonical `resolve_cookie_secure(&WebConfig, &HeaderMap)` helper
/// lives in `auth.rs`. When it exposes a `state.cookie_secure_resolved`
/// snapshot, prefer that over this local mirror.
async fn resolve_https_context(cfg: &ConfigState, xfp_is_https: bool) -> bool {
    if xfp_is_https {
        return true;
    }
    let config = cfg.config.read().await;
    config
        .web
        .cors_origins
        .iter()
        .any(|o| o.starts_with("https://"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    #[test]
    fn dashboard_response_gets_csp_xfo_referrer_nosniff() {
        let mut h = HeaderMap::new();
        apply_headers(&mut h, false, false);
        assert_eq!(
            h.get("content-security-policy").unwrap().to_str().unwrap(),
            CSP
        );
        assert_eq!(
            h.get("x-frame-options").unwrap().to_str().unwrap(),
            "SAMEORIGIN"
        );
        assert_eq!(
            h.get("referrer-policy").unwrap().to_str().unwrap(),
            "strict-origin"
        );
        assert_eq!(
            h.get("x-content-type-options").unwrap().to_str().unwrap(),
            "nosniff"
        );
        assert!(h.get("strict-transport-security").is_none());
    }

    #[test]
    fn proxy_response_drops_csp_and_xfo_but_keeps_referrer_no_referrer() {
        let mut h = HeaderMap::new();
        apply_headers(&mut h, true, false);
        assert!(h.get("content-security-policy").is_none());
        assert!(h.get("x-frame-options").is_none());
        assert_eq!(
            h.get("referrer-policy").unwrap().to_str().unwrap(),
            "no-referrer"
        );
        assert_eq!(
            h.get("x-content-type-options").unwrap().to_str().unwrap(),
            "nosniff"
        );
    }

    #[test]
    fn hsts_present_only_when_https_context_true() {
        let mut h = HeaderMap::new();
        apply_headers(&mut h, false, true);
        assert_eq!(
            h.get("strict-transport-security").unwrap().to_str().unwrap(),
            HSTS_VALUE
        );

        let mut h = HeaderMap::new();
        apply_headers(&mut h, true, true);
        assert_eq!(
            h.get("strict-transport-security").unwrap().to_str().unwrap(),
            HSTS_VALUE
        );
    }
}
