// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Auth HTTP handlers.
//!
//! Split per §8 audit (formerly a single 997-LOC file). Each submodule owns
//! one cohesive flow; this `mod.rs` only re-exports the public handlers used
//! by the router in `server.rs` and the cross-handler test fixtures.

mod login;
mod me;
mod password;
mod register;
mod status;

pub use login::{LoginBody, login, logout};
pub use me::me;
pub use password::{ChangePasswordBody, RecoverBody, change_password, recover, regenerate_recovery_codes};
pub use register::{RegisterBody, register};
pub use status::{AuthStatus, auth_status};

/// Plan-02 AC-3: per-user lockout threshold and window.
///
/// 5 failed attempts within a 10-minute window locks the account until the
/// **oldest** failure ages out (sliding window — admins can short-circuit via
/// `POST /api/admin/users/{id}/unlock`).
pub(super) const LOCKOUT_THRESHOLD: i64 = 5;
pub(super) const LOCKOUT_WINDOW_SECS: i64 = 600;

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::server::build_router;
    use crate::test_helpers::{register_and_login, test_state_with_db};

    #[tokio::test]
    async fn auth_status_setup_required_when_no_users() {
        // A fresh DB with no registered users: auth is enabled, setup is required.
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
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["dashboard_auth_enabled"], true);
        assert_eq!(json["setup_required"], true);
        // Phase 0 mirrored fields (04_architecture.md §1.3) — test_state_with_db
        // seeds an empty default `SystemStatus`: provider=claude, github=missing,
        // no warnings → degraded=false.
        assert_eq!(json["provider_selected"], "claude");
        assert_eq!(json["github_mode"], "missing");
        assert_eq!(json["degraded"], false);
    }

    #[tokio::test]
    async fn auth_status_enabled_when_user_registered() {
        let state = test_state_with_db();
        let _cookie = register_and_login(&state).await;
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
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["dashboard_auth_enabled"], true);
        assert_eq!(json["multi_user"], true);
        assert_eq!(json["setup_required"], false);
        // Phase 0 mirrored fields present even after first-user registration.
        assert_eq!(json["provider_selected"], "claude");
        assert_eq!(json["github_mode"], "missing");
        assert_eq!(json["degraded"], false);
    }

    #[tokio::test]
    async fn login_with_correct_credentials_returns_204_with_cookie() {
        let state = test_state_with_db();
        let _cookie = register_and_login(&state).await;

        // Login again to verify the flow independently.
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::post("/api/auth/login")
                    .header("Content-Type", "application/json")
                    .header("Origin", "http://localhost:8080")
                    .body(Body::from(
                        r#"{"username":"admin","password":"testpassword1234"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        let set_cookie = resp
            .headers()
            .get("set-cookie")
            .expect("expected set-cookie header")
            .to_str()
            .unwrap();
        assert!(
            set_cookie.contains("maestro_session"),
            "cookie should contain maestro_session, got: {set_cookie}"
        );
    }

    #[tokio::test]
    async fn login_with_wrong_credentials_returns_401() {
        let state = test_state_with_db();
        let _cookie = register_and_login(&state).await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::post("/api/auth/login")
                    .header("Content-Type", "application/json")
                    .header("Origin", "http://localhost:8080")
                    .body(Body::from(r#"{"username":"admin","password":"wrong"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn logout_returns_204() {
        let state = test_state_with_db();
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::post("/api/auth/logout")
                    .header("Origin", "http://localhost:8080")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn protected_route_returns_401_without_cookie_when_auth_enabled() {
        let state = test_state_with_db();
        let _cookie = register_and_login(&state).await;

        let app = build_router(state);
        let resp = app
            .oneshot(Request::get("/api/config").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn protected_route_accessible_with_valid_session() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::get("/api/config")
                    .header("Cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
