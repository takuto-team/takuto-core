// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum_extra::extract::cookie::{CookieJar, SameSite};
use cookie::Cookie;
use cookie::time::Duration;
use serde::{Deserialize, Serialize};

use crate::auth::{SESSION_COOKIE_NAME, SESSION_TTL_SECS, credentials_match, sign_session};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct LoginBody {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct AuthStatus {
    pub dashboard_auth_enabled: bool,
}

/// Public probe: whether the server requires dashboard login (reads live config).
pub async fn auth_status(State(state): State<AppState>) -> Json<AuthStatus> {
    let enabled = state.config.read().await.web.dashboard_auth_enabled();
    Json(AuthStatus {
        dashboard_auth_enabled: enabled,
    })
}

/// Set HttpOnly session cookie (same-origin fetch and WebSocket send it automatically).
pub async fn login(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(body): Json<LoginBody>,
) -> impl IntoResponse {
    let cfg = state.config.read().await;
    if !cfg.web.dashboard_auth_enabled() {
        return (
            StatusCode::BAD_REQUEST,
            "dashboard auth not configured (set [web] dashboard_username and dashboard_password)",
        )
            .into_response();
    }
    if !credentials_match(&cfg.web, &body.username, &body.password) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let canonical_user = cfg.web.dashboard_username.trim().to_string();
    let password = cfg.web.dashboard_password.clone();
    drop(cfg);

    let Some(token) = sign_session(&canonical_user, &password) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };

    let cookie = Cookie::build((SESSION_COOKIE_NAME, token))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .max_age(Duration::seconds(SESSION_TTL_SECS as i64))
        .build();

    (jar.add(cookie), StatusCode::NO_CONTENT).into_response()
}

pub async fn logout(jar: CookieJar) -> impl IntoResponse {
    let mut c = Cookie::build((SESSION_COOKIE_NAME, ""))
        .path("/")
        .max_age(Duration::seconds(0))
        .build();
    c.make_removal();
    (jar.add(c), StatusCode::NO_CONTENT).into_response()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tokio::sync::RwLock;
    use tower::ServiceExt;

    use maestro_core::actions::dry_run::DryRunActions;
    use maestro_core::config::{Config, TicketingSystem};
    use maestro_core::workflow::engine::WorkflowEngine;

    use crate::server::build_router;
    use crate::state::AppState;

    fn test_state_no_auth() -> AppState {
        let config = Arc::new(RwLock::new(Config::default()));
        let actions: Arc<dyn maestro_core::actions::traits::ExternalActions> = Arc::new(
            DryRunActions::new(std::env::temp_dir(), "origin".to_string(), None),
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
            polling_paused: Arc::new(AtomicBool::new(false)),
            jira_available,
            ticketing_system: TicketingSystem::None,
            editor_scanners: Arc::new(RwLock::new(HashMap::new())),
            dynamic_forwards: Arc::new(RwLock::new(HashMap::new())),
            terminal_ports: Arc::new(RwLock::new(HashMap::new())),
            run_commands: Arc::new(RwLock::new(HashMap::new())),
            preflight_error: None,
            config_path: std::env::temp_dir().join("config.toml"),
            config_writer: None,
        }
    }

    fn test_state_with_auth() -> AppState {
        let mut cfg = Config::default();
        cfg.web.dashboard_username = "admin".to_string();
        cfg.web.dashboard_password = "secret123".to_string();
        let config = Arc::new(RwLock::new(cfg));
        let actions: Arc<dyn maestro_core::actions::traits::ExternalActions> = Arc::new(
            DryRunActions::new(std::env::temp_dir(), "origin".to_string(), None),
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
            polling_paused: Arc::new(AtomicBool::new(false)),
            jira_available,
            ticketing_system: TicketingSystem::None,
            editor_scanners: Arc::new(RwLock::new(HashMap::new())),
            dynamic_forwards: Arc::new(RwLock::new(HashMap::new())),
            terminal_ports: Arc::new(RwLock::new(HashMap::new())),
            run_commands: Arc::new(RwLock::new(HashMap::new())),
            preflight_error: None,
            config_path: std::env::temp_dir().join("config.toml"),
            config_writer: None,
        }
    }

    #[tokio::test]
    async fn auth_status_disabled_when_no_credentials() {
        let state = test_state_no_auth();
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
        assert_eq!(json["dashboard_auth_enabled"], false);
    }

    #[tokio::test]
    async fn auth_status_enabled_when_credentials_set() {
        let state = test_state_with_auth();
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
    }

    #[tokio::test]
    async fn login_with_correct_credentials_returns_204_with_cookie() {
        let state = test_state_with_auth();
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::post("/api/auth/login")
                    .header("Content-Type", "application/json")
                    .body(Body::from(
                        r#"{"username":"admin","password":"secret123"}"#,
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
        let state = test_state_with_auth();
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::post("/api/auth/login")
                    .header("Content-Type", "application/json")
                    .body(Body::from(
                        r#"{"username":"admin","password":"wrong"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn logout_returns_204() {
        let state = test_state_no_auth();
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::post("/api/auth/logout")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn protected_route_returns_401_without_cookie_when_auth_enabled() {
        let state = test_state_with_auth();
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
    }

    #[tokio::test]
    async fn protected_route_accessible_with_valid_session() {
        let state = test_state_with_auth();

        // First, login to get a session cookie.
        let app = build_router(state.clone());
        let login_resp = app
            .oneshot(
                Request::post("/api/auth/login")
                    .header("Content-Type", "application/json")
                    .body(Body::from(
                        r#"{"username":"admin","password":"secret123"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(login_resp.status(), StatusCode::NO_CONTENT);
        let set_cookie = login_resp
            .headers()
            .get("set-cookie")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        // Extract just the cookie name=value pair (before the first `;`).
        let cookie_val = set_cookie.split(';').next().unwrap().trim();

        // Now use that cookie to access a protected route.
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::get("/api/config")
                    .header("Cookie", cookie_val)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
