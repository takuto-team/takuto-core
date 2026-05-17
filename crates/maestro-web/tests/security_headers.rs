// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

// Copyright (C) 2026 Alexandre Obellianne
//
// Integration tests for plan-02 AC-6 — Defence-in-depth response headers.
//
// Verifies that the `security_headers_middleware` (outermost layer on the
// top-level router) injects:
//   - `Content-Security-Policy` (with `frame-ancestors 'self'`)
//   - `X-Frame-Options: SAMEORIGIN`
//   - `Referrer-Policy: strict-origin`
//   - `X-Content-Type-Options: nosniff`
//   - `Strict-Transport-Security` only when `cookie_secure` resolves true
//     (auto-detect: `X-Forwarded-Proto: https` or any `https://` cors origin)
//
// And that for `/s/*` proxy responses the policy is loosened: no CSP, no
// X-Frame-Options, and `Referrer-Policy` is overridden to `no-referrer`.
//
// The tests deliberately use the public `/api/auth/status` route (no session
// cookie required) so they don't depend on the login flow being fully wired
// up by teammate work (Dev B's rate-limit + Dev C's secure-cookie path).
// Headers are emitted by the outermost middleware regardless of response
// status, so a 401 response is just as good as a 200 for assertions about
// header presence.
//
// See `tmp/plan-02-acceptance.md` AC-6 G/W/T 6.1–6.6.

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use tower::ServiceExt;

use maestro_web::server::build_router;
use maestro_web::test_helpers::test_state_with_db;

/// G/W/T 6.1 — Authenticated dashboard response carries CSP.
#[tokio::test]
async fn dashboard_response_has_csp() {
    let state = test_state_with_db();
    let app = build_router(state);
    let resp = app
        .oneshot(
            Request::get("/api/auth/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let csp = resp
        .headers()
        .get("content-security-policy")
        .expect("CSP header must be present on dashboard responses")
        .to_str()
        .unwrap();
    assert!(
        csp.contains("frame-ancestors 'self'"),
        "CSP must contain `frame-ancestors 'self'`; got: {csp}"
    );
    assert!(csp.contains("default-src 'self'"));
}

/// G/W/T 6.1 — `X-Frame-Options: SAMEORIGIN`.
#[tokio::test]
async fn dashboard_response_has_x_frame_options() {
    let state = test_state_with_db();
    let app = build_router(state);
    let resp = app
        .oneshot(
            Request::get("/api/auth/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.headers().get("x-frame-options").unwrap().to_str().unwrap(),
        "SAMEORIGIN"
    );
}

/// G/W/T 6.1 — `Referrer-Policy: strict-origin` on dashboard responses.
#[tokio::test]
async fn dashboard_response_has_referrer_policy() {
    let state = test_state_with_db();
    let app = build_router(state);
    let resp = app
        .oneshot(
            Request::get("/api/auth/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.headers().get("referrer-policy").unwrap().to_str().unwrap(),
        "strict-origin"
    );
}

/// G/W/T 6.1 — `X-Content-Type-Options: nosniff` on every response.
#[tokio::test]
async fn dashboard_response_has_x_content_type_options() {
    let state = test_state_with_db();
    let app = build_router(state);
    let resp = app
        .oneshot(
            Request::get("/api/auth/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.headers().get("x-content-type-options").unwrap().to_str().unwrap(),
        "nosniff"
    );
}

/// G/W/T 6.3 — Error responses (401 from auth) also get all the security
/// headers. The middleware is the outermost layer so it wraps rejections too.
#[tokio::test]
async fn unauthenticated_401_response_has_security_headers() {
    let state = test_state_with_db();
    let app = build_router(state);
    let resp = app
        .oneshot(
            Request::get("/api/config")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(
        resp.headers().get("content-security-policy").is_some(),
        "CSP must be set on 401 responses too"
    );
    assert!(resp.headers().get("x-frame-options").is_some());
    assert!(resp.headers().get("x-content-type-options").is_some());
}

/// G/W/T 6.3 — 403 responses from the CSRF middleware also carry the headers.
#[tokio::test]
async fn csrf_403_response_has_security_headers() {
    let state = test_state_with_db();
    let app = build_router(state);
    let resp = app
        .oneshot(
            Request::post("/api/workflows/start-manual")
                .header("Content-Type", "application/json")
                .body(Body::from(r#"{"ticket_key":"X-1"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert!(
        resp.headers().get("content-security-policy").is_some(),
        "CSP must be set on 403 CSRF rejections too"
    );
    assert!(resp.headers().get("x-content-type-options").is_some());
}

/// G/W/T 6.2 — `Strict-Transport-Security` is NOT emitted when the deployment
/// is plain HTTP (default test config: empty cors_origins, no
/// X-Forwarded-Proto).
#[tokio::test]
async fn hsts_absent_when_not_https() {
    let state = test_state_with_db();
    let app = build_router(state);
    let resp = app
        .oneshot(
            Request::get("/api/auth/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        resp.headers().get("strict-transport-security").is_none(),
        "HSTS must not be emitted in a plain-HTTP context"
    );
}

/// G/W/T 6.2 — When the request carries `X-Forwarded-Proto: https`, HSTS is
/// emitted on the response (a TLS-terminating proxy indicates the public
/// connection is HTTPS).
#[tokio::test]
async fn hsts_emitted_when_x_forwarded_proto_https() {
    let state = test_state_with_db();
    let app = build_router(state);
    let resp = app
        .oneshot(
            Request::get("/api/auth/status")
                .header("X-Forwarded-Proto", "https")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let hsts = resp
        .headers()
        .get("strict-transport-security")
        .expect("HSTS must be emitted when X-Forwarded-Proto: https")
        .to_str()
        .unwrap();
    assert!(
        hsts.contains("max-age=31536000"),
        "HSTS must use 1y max-age; got: {hsts}"
    );
    assert!(
        hsts.contains("includeSubDomains"),
        "HSTS must include subdomains; got: {hsts}"
    );
}

/// G/W/T 6.2 — When `cors_origins` contains an `https://` entry, HSTS is also
/// emitted (auto-detection for production deployments where the operator
/// configured the public origin without setting `cookie_secure` explicitly).
#[tokio::test]
async fn hsts_emitted_when_cors_origins_has_https() {
    let state = test_state_with_db();
    {
        let mut cfg = state.config.write().await;
        cfg.web.cors_origins = vec!["https://maestro.example.com".to_string()];
    }
    let app = build_router(state);
    let resp = app
        .oneshot(
            Request::get("/api/auth/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let hsts = resp
        .headers()
        .get("strict-transport-security")
        .expect("HSTS must be emitted when any cors_origins entry is https://");
    assert!(hsts.to_str().unwrap().contains("max-age=31536000"));
}

/// G/W/T 6.6 — OPTIONS preflight responses also carry the headers (since the
/// middleware is outermost and runs on every response, including the ones
/// short-circuited by the CORS layer).
#[tokio::test]
async fn options_preflight_has_security_headers() {
    let state = test_state_with_db();
    let app = build_router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .method(Method::OPTIONS)
                .uri("/api/auth/status")
                .header("Origin", "http://localhost:8080")
                .header("Access-Control-Request-Method", "GET")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        resp.headers().get("x-content-type-options").is_some(),
        "OPTIONS preflight must still get security headers"
    );
}

/// G/W/T 6.5 — `/s/*` proxy responses do NOT carry `Content-Security-Policy`
/// (the editor uses inline scripts which would be broken by the dashboard
/// CSP), but DO carry `X-Content-Type-Options: nosniff` and the tighter
/// `Referrer-Policy: no-referrer` (the path token is sensitive).
///
/// We don't need a live proxy backend: any GET to `/s/<random>/...` is
/// dispatched to the proxy handler which returns a 404 because the token
/// isn't registered. The middleware still runs on that response.
#[tokio::test]
async fn proxy_path_no_csp_but_has_nosniff_and_no_referrer() {
    let state = test_state_with_db();
    let app = build_router(state);
    let resp = app
        .oneshot(
            Request::get("/s/aaaa1111bbbb2222cccc3333dddd4444/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        resp.headers().get("content-security-policy").is_none(),
        "/s/* responses must NOT carry CSP"
    );
    assert!(
        resp.headers().get("x-frame-options").is_none(),
        "/s/* responses must NOT carry X-Frame-Options"
    );
    assert_eq!(
        resp.headers().get("referrer-policy").unwrap().to_str().unwrap(),
        "no-referrer",
        "/s/* must override Referrer-Policy to no-referrer"
    );
    assert_eq!(
        resp.headers().get("x-content-type-options").unwrap().to_str().unwrap(),
        "nosniff"
    );
}

/// Smoke test — the middleware should not duplicate headers if it accidentally
/// runs twice (multiple `.layer(...)` calls). axum's HeaderMap.insert
/// semantics replace existing values, so this is effectively asserting we
/// don't end up with comma-joined duplicates.
#[tokio::test]
async fn headers_are_not_duplicated() {
    let state = test_state_with_db();
    let app = build_router(state);
    let resp = app
        .oneshot(
            Request::get("/api/auth/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    for header in &[
        "content-security-policy",
        "x-frame-options",
        "referrer-policy",
        "x-content-type-options",
    ] {
        let count = resp.headers().get_all(*header).iter().count();
        assert_eq!(count, 1, "header {header} appeared {count} times");
    }
}
