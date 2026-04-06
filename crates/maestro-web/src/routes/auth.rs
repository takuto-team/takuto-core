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
