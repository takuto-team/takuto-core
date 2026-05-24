// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

// Copyright (C) 2026 Alexandre Obellianne
//
// Integration tests for plan-02 AC-2 — `Secure` cookie flag with auto-detect.
//
// Resolution rule (from `crates/maestro-web/src/auth.rs::resolve_cookie_secure`):
//   1. Explicit `[web] cookie_secure = Some(v)` wins.
//   2. Else if any `cors_origins` entry is `https://…` → true.
//   3. Else if request carries `X-Forwarded-Proto: https` → true.
//   4. Otherwise → false.
//
// Each test drives a full `POST /api/auth/login` through `build_router` and
// parses the `Set-Cookie` header for the presence (or absence) of `Secure`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use maestro_web::server::build_router;
use maestro_web::state::AppState;
use maestro_web::test_helpers::{TEST_ORIGIN, register_and_login, test_state_with_db};

/// Extract the `Set-Cookie` header for `maestro_session` from the response.
/// Panics if no such header is present.
fn session_set_cookie(resp: &axum::http::Response<Body>) -> String {
    for v in resp.headers().get_all("set-cookie").iter() {
        let s = v.to_str().unwrap();
        if s.starts_with("maestro_session=") {
            return s.to_string();
        }
    }
    panic!("no maestro_session Set-Cookie header on response");
}

/// Returns `true` when the `Set-Cookie` string carries the `Secure` attribute.
fn cookie_has_secure(set_cookie: &str) -> bool {
    set_cookie
        .split(';')
        .map(str::trim)
        .any(|attr| attr.eq_ignore_ascii_case("Secure"))
}

/// Send `POST /api/auth/login` for the admin user, optionally adding extra headers.
async fn login_request(state: &AppState, extra_headers: &[(&str, &str)]) -> axum::http::Response<Body> {
    let app = build_router(state.clone());
    let mut req = Request::post("/api/auth/login")
        .header("Content-Type", "application/json")
        .header("Origin", TEST_ORIGIN);
    for (k, v) in extra_headers {
        req = req.header(*k, *v);
    }
    let req = req
        .body(Body::from(
            r#"{"username":"admin","password":"testpassword1234"}"#,
        ))
        .unwrap();
    app.oneshot(req).await.unwrap()
}

#[tokio::test]
async fn cookie_no_secure_when_plain_http() {
    // G/W/T 2.1: default config + no X-Forwarded-Proto → no `Secure`.
    let state = test_state_with_db();
    let _ = register_and_login(&state).await;

    let resp = login_request(&state, &[]).await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let set_cookie = session_set_cookie(&resp);
    assert!(
        !cookie_has_secure(&set_cookie),
        "expected NO Secure attribute on plain-HTTP cookie, got: {set_cookie}",
    );
}

#[tokio::test]
async fn cookie_secure_when_configured_true() {
    // G/W/T 2.2: explicit `cookie_secure = true` → `Secure` present.
    let state = test_state_with_db();
    let _ = register_and_login(&state).await;

    {
        let mut cfg = state.config().config.write().await;
        cfg.web.cookie_secure = Some(true);
    }

    let resp = login_request(&state, &[]).await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let set_cookie = session_set_cookie(&resp);
    assert!(
        cookie_has_secure(&set_cookie),
        "expected Secure attribute when cookie_secure = true, got: {set_cookie}",
    );
}

#[tokio::test]
async fn cookie_secure_when_https_origin_present() {
    // G/W/T 2.3: cors_origins contains https://… → `Secure` present (auto-detect).
    //
    // We seed the allowlist with BOTH the test origin and the https origin.
    // `resolved_cors_origins()` only auto-fills the host/port-derived defaults
    // when the explicit list is empty — as soon as we set a single entry, we
    // must include the test origin or CSRF (plan-02 AC-1) rejects the
    // `Origin: http://localhost:8080` request with 403.
    let state = test_state_with_db();
    let _ = register_and_login(&state).await;

    {
        let mut cfg = state.config().config.write().await;
        cfg.web.cors_origins = vec![
            TEST_ORIGIN.into(),
            "https://maestro.example.com".into(),
        ];
    }

    let resp = login_request(&state, &[]).await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let set_cookie = session_set_cookie(&resp);
    assert!(
        cookie_has_secure(&set_cookie),
        "expected Secure attribute when an https:// origin is allowed, got: {set_cookie}",
    );
}

#[tokio::test]
async fn cookie_secure_when_forwarded_proto_https() {
    // G/W/T 2.4: request has `X-Forwarded-Proto: https` → `Secure` present.
    let state = test_state_with_db();
    let _ = register_and_login(&state).await;

    let resp = login_request(&state, &[("X-Forwarded-Proto", "https")]).await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let set_cookie = session_set_cookie(&resp);
    assert!(
        cookie_has_secure(&set_cookie),
        "expected Secure attribute when X-Forwarded-Proto: https, got: {set_cookie}",
    );
}

#[tokio::test]
async fn explicit_cookie_secure_false_overrides_auto_detect() {
    // G/W/T 2.5: explicit `cookie_secure = false` beats both signals.
    //
    // Seed the allowlist with both the test origin and the https origin — see
    // note on `cookie_secure_when_https_origin_present` for the CSRF rationale.
    let state = test_state_with_db();
    let _ = register_and_login(&state).await;

    {
        let mut cfg = state.config().config.write().await;
        cfg.web.cookie_secure = Some(false);
        cfg.web.cors_origins = vec![
            TEST_ORIGIN.into(),
            "https://maestro.example.com".into(),
        ];
    }

    let resp = login_request(&state, &[("X-Forwarded-Proto", "https")]).await;
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let set_cookie = session_set_cookie(&resp);
    assert!(
        !cookie_has_secure(&set_cookie),
        "explicit cookie_secure=false must override auto-detect, got: {set_cookie}",
    );
}

#[tokio::test]
async fn logout_cookie_inherits_secure_resolution() {
    // G/W/T 2.6: logout's removal cookie must also carry Secure when configured.
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;

    {
        let mut cfg = state.config().config.write().await;
        cfg.web.cookie_secure = Some(true);
    }

    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::post("/api/auth/logout")
                .header("Origin", TEST_ORIGIN)
                .header("Cookie", &admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let set_cookie = session_set_cookie(&resp);
    assert!(
        cookie_has_secure(&set_cookie),
        "logout removal cookie must carry Secure when cookie_secure=true, got: {set_cookie}",
    );
}
