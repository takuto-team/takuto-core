// Copyright (C) 2026 Alexandre Obellianne
//
// Integration tests for AC-3 — admin gates on shared-state endpoints.
//
// For every route listed in `tmp/plan-01-acceptance.md` AC-3:
//   - PUT  /api/config
//   - POST /api/config/reload
//   - POST /api/polling/pause
//   - POST /api/polling/resume
//   - POST /api/workspaces/switch
//   - POST /api/repos/clone
//
// we assert:
//   1. A non-admin (`role = user`) caller is rejected with `403 FORBIDDEN`.
//   2. An admin caller is NOT rejected (i.e. the response is anything other
//      than 403 — the handler is reached). Concretely most routes return
//      200/202/204 on a happy path; a few return 4xx for unrelated reasons
//      (missing config file, invalid body, missing workspace). What matters
//      for AC-3 is only that the admin gate did not block the request.
//
// We also confirm the read-side endpoints (`GET /api/config`,
// `GET /api/polling`) stay open to any authenticated user.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use maestro_web::server::build_router;
use maestro_web::state::AppState;
use maestro_web::test_helpers::{TEST_ORIGIN, register_and_login, test_state_with_db};

/// Create a second user (`role = user`) via the admin API and log them in.
///
/// The first registered user becomes admin automatically (see
/// `db::users::create_user`'s first-user-becomes-admin rule), so this helper
/// uses the admin cookie to create the second user via `POST /api/users`,
/// then logs them in to obtain their own session cookie.
async fn create_and_login_regular_user(
    state: &AppState,
    admin_cookie: &str,
    username: &str,
    password: &str,
) -> String {
    // Create the user as admin.
    let app = build_router(state.clone());
    let body = format!(
        r#"{{"username":"{username}","password":"{password}","role":"user"}}"#,
    );
    let resp = app
        .oneshot(
            Request::post("/api/users")
                .header("Content-Type", "application/json")
                .header("Origin", TEST_ORIGIN)
                .header("Cookie", admin_cookie)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "creating regular user should succeed"
    );

    // Login the new user.
    let app = build_router(state.clone());
    let body = format!(r#"{{"username":"{username}","password":"{password}"}}"#);
    let login_resp = app
        .oneshot(
            Request::post("/api/auth/login")
                .header("Content-Type", "application/json")
                .header("Origin", TEST_ORIGIN)
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        login_resp.status(),
        StatusCode::NO_CONTENT,
        "login should succeed"
    );
    let set_cookie = login_resp
        .headers()
        .get("set-cookie")
        .expect("login should set a cookie")
        .to_str()
        .unwrap()
        .to_string();
    set_cookie.split(';').next().unwrap().trim().to_string()
}

/// Describe a single admin-gated route under test.
struct Route {
    method: &'static str,
    path: &'static str,
    body: Option<&'static str>,
}

const ADMIN_GATED_ROUTES: &[Route] = &[
    // PUT /api/config with a valid patch body.
    Route {
        method: "PUT",
        path: "/api/config",
        body: Some(r#"{"general":{"max_concurrent_workflows":5}}"#),
    },
    // POST /api/config/reload — body is empty; admin path will 4xx because
    // no config file exists in the test state, but that is still non-403.
    Route {
        method: "POST",
        path: "/api/config/reload",
        body: None,
    },
    // POST /api/polling/pause and /resume — bodies are empty.
    Route {
        method: "POST",
        path: "/api/polling/pause",
        body: None,
    },
    Route {
        method: "POST",
        path: "/api/polling/resume",
        body: None,
    },
    // Plan-10: `/api/workspaces/switch` and `/api/repos/clone` are
    // hard-deleted (decision #3). The cloning + add-repo flow is now the
    // open-to-all `POST /api/repositories`; the legacy admin gates here are
    // no longer applicable.
];

fn build_request(route: &Route, cookie: &str) -> Request<Body> {
    let mut req = Request::builder().method(route.method).uri(route.path);
    req = req.header("Cookie", cookie);
    // CSRF middleware requires `Origin` on every mutating method.
    req = req.header("Origin", TEST_ORIGIN);
    let body = if let Some(b) = route.body {
        req = req.header("Content-Type", "application/json");
        Body::from(b)
    } else {
        Body::empty()
    };
    req.body(body).unwrap()
}

#[tokio::test]
async fn non_admin_is_forbidden_on_every_mutating_route() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;
    let user_cookie =
        create_and_login_regular_user(&state, &admin_cookie, "bob", "secretpassword123!@#").await;

    for route in ADMIN_GATED_ROUTES {
        let app = build_router(state.clone());
        let resp = app
            .oneshot(build_request(route, &user_cookie))
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "non-admin should be 403 on {} {}; got {}",
            route.method,
            route.path,
            resp.status()
        );
    }
}

#[tokio::test]
async fn admin_is_not_forbidden_on_every_mutating_route() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;

    for route in ADMIN_GATED_ROUTES {
        let app = build_router(state.clone());
        let resp = app
            .oneshot(build_request(route, &admin_cookie))
            .await
            .unwrap();
        assert_ne!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "admin should NOT be 403 on {} {}; got {}",
            route.method,
            route.path,
            resp.status()
        );
        // Also reject 401 so we know auth is being recognised correctly.
        assert_ne!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "admin should NOT be 401 on {} {}",
            route.method,
            route.path
        );
    }
}

#[tokio::test]
async fn get_config_is_open_to_any_authenticated_user() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;
    let user_cookie =
        create_and_login_regular_user(&state, &admin_cookie, "alice", "secretpassword123!@#").await;

    for cookie in &[admin_cookie.as_str(), user_cookie.as_str()] {
        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::get("/api/config")
                    .header("Cookie", *cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "GET /api/config should be 200 for any authenticated user"
        );
    }
}

#[tokio::test]
async fn get_polling_is_open_to_any_authenticated_user() {
    let state = test_state_with_db();
    let admin_cookie = register_and_login(&state).await;
    let user_cookie =
        create_and_login_regular_user(&state, &admin_cookie, "carol", "secretpassword123!@#").await;

    for cookie in &[admin_cookie.as_str(), user_cookie.as_str()] {
        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::get("/api/polling")
                    .header("Cookie", *cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "GET /api/polling should be 200 for any authenticated user"
        );
    }
}
