// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

// Copyright (C) 2026 Alexandre Obellianne
//
// Integration tests for CSRF via Origin/Referer allowlist.
//
// The middleware MUST:
//   - reject `POST/PUT/DELETE/PATCH` with a missing or unlisted `Origin` (or
//     `Referer` fallback) with **403 Forbidden** before any handler runs;
//   - allow `GET/HEAD/OPTIONS` unconditionally;
//   - allow `/s/*` proxy traffic (its own auth gate handles those paths);
//   - gate `/api/auth/login` too — a planted cross-origin page must NOT be
//     able to POST a login on behalf of the user.
//
// CSRF is the OUTERMOST middleware on `api_public`/`api_protected`, so it
// rejects before the auth middleware (and before the per-IP rate-limit
// layer on `/auth/login`). That lets these tests cover the rejection paths
// without driving a full register-then-login flow.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use maestro_web::server::build_router;
use maestro_web::test_helpers::{TEST_ORIGIN, test_state_with_db};

/// G/W/T 1.1 — POST with no `Origin` and no `Referer` is **403** before any
/// handler (or auth lookup) runs.
///
/// Hits a protected route without a cookie. With CSRF active, the middleware
/// rejects with 403 before the auth middleware can return 401. Pre-CSRF
/// behaviour would have been 401, so this assertion uniquely catches a
/// regression where the CSRF layer is missing or misordered.
#[tokio::test]
async fn csrf_post_without_origin_is_403() {
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

    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "expected 403 from CSRF; got {} — middleware likely missing or misordered",
        resp.status()
    );
}

/// G/W/T 1.2 — POST with an `Origin` that does not match the allowlist is 403.
#[tokio::test]
async fn csrf_post_with_bad_origin_is_403() {
    let state = test_state_with_db();

    let app = build_router(state);
    let resp = app
        .oneshot(
            Request::post("/api/workflows/start-manual")
                .header("Content-Type", "application/json")
                .header("Origin", "https://evil.example")
                .body(Body::from(r#"{"ticket_key":"X-1"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "expected 403, got {}",
        resp.status()
    );
}

/// G/W/T 1.3 — POST with a matching `Origin` is NOT rejected by the CSRF
/// layer. We hit a protected route without a cookie, so auth returns 401 —
/// which is fine: the only thing under test here is that CSRF didn't 403.
#[tokio::test]
async fn csrf_post_with_good_origin_passes_csrf_layer() {
    let state = test_state_with_db();

    let app = build_router(state);
    let resp = app
        .oneshot(
            Request::post("/api/workflows/start-manual")
                .header("Content-Type", "application/json")
                .header("Origin", TEST_ORIGIN)
                .body(Body::from(r#"{"ticket_key":"X-1"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_ne!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "good Origin must not be rejected by CSRF"
    );
    // The auth middleware should be the rejecter (no cookie → 401).
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "expected 401 from auth layer, got {}",
        resp.status()
    );
}

/// G/W/T 1.5 — GET requests are exempt from the CSRF check; the middleware
/// short-circuits safe methods before inspecting `Origin`. Use the public
/// `/api/auth/status` endpoint so no cookie is required.
#[tokio::test]
async fn csrf_get_without_origin_passes() {
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
        resp.status(),
        StatusCode::OK,
        "GET on a public route without Origin must pass; got {}",
        resp.status()
    );
}

/// G/W/T 1.4 — When `Origin` is absent the middleware falls back to extracting
/// the origin from `Referer`. A `Referer` whose scheme+host+port matches the
/// allowlist passes the CSRF check (auth layer below may still 401).
#[tokio::test]
async fn csrf_uses_referer_when_origin_absent() {
    let state = test_state_with_db();

    let app = build_router(state);
    let resp = app
        .oneshot(
            Request::post("/api/workflows/start-manual")
                .header("Content-Type", "application/json")
                .header("Referer", format!("{TEST_ORIGIN}/dashboard/workflows"))
                .body(Body::from(r#"{"ticket_key":"X-1"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_ne!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "Referer fallback with allowed origin must not 403; got {}",
        resp.status()
    );
}

/// G/W/T 1.6 — `/api/auth/login` is also gated by CSRF, before auth runs.
/// CSRF is the OUTERMOST layer on `api_public`, so a bad-origin login
/// attempt is rejected before any rate-limit, key-extractor, or handler runs
/// — the response is 403, not 401, 429, or 500.
#[tokio::test]
async fn csrf_applies_to_login() {
    let state = test_state_with_db();

    let app = build_router(state);
    let resp = app
        .oneshot(
            Request::post("/api/auth/login")
                .header("Content-Type", "application/json")
                .header("Origin", "https://evil.example")
                .body(Body::from(
                    r#"{"username":"admin","password":"testpassword1234"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "bad-origin login must be 403 (NOT 401/429/500); got {}",
        resp.status()
    );
    // No session cookie issued.
    assert!(
        resp.headers().get("set-cookie").is_none(),
        "rejected login must not set a cookie"
    );
}

/// G/W/T 1.7 — `/s/*` is NOT gated by CSRF. The shared-port reverse proxy
/// authenticates by opaque path token, not by dashboard cookie. A bad-origin
/// POST to `/s/<token>/...` must NOT yield 403 from the CSRF layer — it must
/// fall through to the proxy handler (which returns its own 404 for unknown
/// tokens).
#[tokio::test]
async fn csrf_skips_proxy_path() {
    let state = test_state_with_db();

    let app = build_router(state);
    let resp = app
        .oneshot(
            Request::post("/s/aaaa1111bbbb2222cccc3333dddd4444/api/anything")
                .header("Content-Type", "application/json")
                .header("Origin", "https://evil.example")
                .body(Body::from(r#"{"k":"v"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_ne!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "/s/* must not be CSRF-gated; got 403"
    );
}
