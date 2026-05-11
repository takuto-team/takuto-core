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

use crate::auth::{
    SESSION_COOKIE_NAME, SESSION_TTL_SECS, authenticate_db_user, create_db_session,
    credentials_match, delete_db_session, sign_session,
};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct LoginBody {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct AuthStatus {
    pub dashboard_auth_enabled: bool,
    /// `true` when the SQLite database has users (multi-user mode active).
    pub multi_user: bool,
    /// `true` when the database is available but has no users yet (first-user registration required).
    pub setup_required: bool,
}

/// Public probe: whether the server requires dashboard login (reads live config).
pub async fn auth_status(State(state): State<AppState>) -> Json<AuthStatus> {
    let enabled = state.config.read().await.web.dashboard_auth_enabled();

    let (multi_user, setup_required) = if let Some(ref db) = state.db {
        let db = db.clone();
        let count = tokio::task::spawn_blocking(move || {
            let conn = db.conn().blocking_lock();
            maestro_core::db::users::count_users(&conn).unwrap_or(0)
        })
        .await
        .unwrap_or(0);
        (count > 0, count == 0)
    } else {
        (false, false)
    };

    Json(AuthStatus {
        dashboard_auth_enabled: enabled || multi_user,
        multi_user,
        setup_required,
    })
}

/// Set HttpOnly session cookie (same-origin fetch and WebSocket send it automatically).
///
/// Supports two modes:
/// 1. **DB auth** -- when the database has users, authenticate against SQLite and issue a `db-` session.
/// 2. **Legacy auth** -- fall back to config-based HMAC session.
pub async fn login(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(body): Json<LoginBody>,
) -> impl IntoResponse {
    // Try database-backed auth first.
    if let Some(ref db) = state.db {
        let db_clone = db.clone();
        let has_users = tokio::task::spawn_blocking(move || {
            let conn = db_clone.conn().blocking_lock();
            maestro_core::db::users::count_users(&conn).unwrap_or(0) > 0
        })
        .await
        .unwrap_or(false);

        if has_users {
            let user = authenticate_db_user(db, &body.username, &body.password).await;
            let Some(user) = user else {
                return StatusCode::UNAUTHORIZED.into_response();
            };

            // Create a DB session.
            let db_clone = db.clone();
            let user_id = user.id.clone();
            let token = tokio::task::spawn_blocking(move || {
                let conn = db_clone.conn().blocking_lock();
                create_db_session(&conn, &user_id)
            })
            .await;

            let token = match token {
                Ok(Ok(t)) => t,
                _ => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            };

            let cookie = Cookie::build((SESSION_COOKIE_NAME, token))
                .path("/")
                .http_only(true)
                .same_site(SameSite::Lax)
                .max_age(Duration::seconds(SESSION_TTL_SECS as i64))
                .build();

            return (jar.add(cookie), StatusCode::NO_CONTENT).into_response();
        }
    }

    // Legacy config-based auth fallback.
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

pub async fn logout(State(state): State<AppState>, jar: CookieJar) -> impl IntoResponse {
    // If this is a DB session, delete the server-side session record.
    if let Some(ref db) = state.db {
        // Extract the cookie value from the jar.
        if let Some(cookie) = jar.get(SESSION_COOKIE_NAME) {
            let cookie_val = cookie.value().to_string();
            let db = db.clone();
            let _ = tokio::task::spawn_blocking(move || {
                let conn = db.conn().blocking_lock();
                delete_db_session(&conn, &cookie_val);
            })
            .await;
        }
    }

    let mut c = Cookie::build((SESSION_COOKIE_NAME, ""))
        .path("/")
        .max_age(Duration::seconds(0))
        .build();
    c.make_removal();
    (jar.add(c), StatusCode::NO_CONTENT).into_response()
}

// ---------------------------------------------------------------------------
// First-user registration
// ---------------------------------------------------------------------------

/// Request body for first-user registration.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterBody {
    pub username: String,
    pub password: String,
}

/// Registration response containing recovery codes.
#[derive(Debug, Serialize)]
struct RegisterResponse {
    user_id: String,
    username: String,
    role: String,
    recovery_codes: Vec<String>,
}

/// Register the first user (admin) when the database exists but has no users.
///
/// Returns **201 Created** with recovery codes on success. Only available when
/// `state.db` is `Some` and the users table is empty.
pub async fn register(
    State(state): State<AppState>,
    Json(body): Json<RegisterBody>,
) -> impl IntoResponse {
    let Some(ref db) = state.db else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Database not available"})),
        )
            .into_response();
    };

    let db = db.clone();
    let username = body.username;
    let password = body.password;

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.conn().blocking_lock();

        // Only allow registration when no users exist (first-user setup).
        let count = maestro_core::db::users::count_users(&conn)?;
        if count > 0 {
            return Err(maestro_core::error::MaestroError::Auth(
                "Registration is closed: users already exist. Use admin API to create new users."
                    .into(),
            ));
        }

        if username.trim().is_empty() {
            return Err(maestro_core::error::MaestroError::Auth(
                "Username cannot be empty".into(),
            ));
        }
        if password.is_empty() {
            return Err(maestro_core::error::MaestroError::Auth(
                "Password cannot be empty".into(),
            ));
        }

        // Create admin user.
        let user = maestro_core::db::users::create_user(
            &conn,
            &username,
            maestro_core::db::models::UserRole::Admin,
        )?;

        // Store password.
        maestro_core::db::credentials::store_password(&conn, &user.id, &password)?;

        // Generate recovery codes.
        let codes =
            maestro_core::db::credentials::generate_recovery_codes(&conn, &user.id, 8)?;

        Ok(RegisterResponse {
            user_id: user.id,
            username: user.username,
            role: user.role.as_str().to_string(),
            recovery_codes: codes,
        })
    })
    .await;

    match result {
        Ok(Ok(resp)) => (StatusCode::CREATED, Json(serde_json::json!(resp))).into_response(),
        Ok(Err(e)) => {
            let msg = e.to_string();
            if msg.contains("already exist") || msg.contains("Registration is closed") {
                (
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({"error": msg})),
                )
                    .into_response()
            } else {
                (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": msg})),
                )
                    .into_response()
            }
        }
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "Internal server error"})),
        )
            .into_response(),
    }
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
            db: None,
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
            clone_in_progress: Arc::new(AtomicBool::new(false)),
            path_token_registry: crate::session_registry::PathTokenRegistry::new(),
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
            db: None,
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
            clone_in_progress: Arc::new(AtomicBool::new(false)),
            path_token_registry: crate::session_registry::PathTokenRegistry::new(),
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
                    .body(Body::from(r#"{"username":"admin","password":"secret123"}"#))
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
                    .body(Body::from(r#"{"username":"admin","password":"wrong"}"#))
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
            .oneshot(Request::get("/api/config").body(Body::empty()).unwrap())
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
                    .body(Body::from(r#"{"username":"admin","password":"secret123"}"#))
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
