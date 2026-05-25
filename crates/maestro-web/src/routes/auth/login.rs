// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! `POST /api/auth/login` and `POST /api/auth/logout`.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum_extra::extract::cookie::{CookieJar, SameSite};
use cookie::Cookie;
use cookie::time::Duration;
use serde::Deserialize;

use crate::auth::{
    SESSION_COOKIE_NAME, SESSION_IDLE_TTL_SECS, authenticate_db_user, create_db_session,
    delete_db_session, now_unix, resolve_cookie_secure,
};
use crate::state::{AuthState, ConfigState};
use maestro_core::db::login_attempts::{
    AttemptKind, clear_failed_attempts, failed_count_in_window, oldest_failure_ts_in_window,
    record_attempt,
};

use super::{LOCKOUT_THRESHOLD, LOCKOUT_WINDOW_SECS};

#[derive(Debug, Deserialize)]
pub struct LoginBody {
    pub username: String,
    pub password: String,
}

/// Set HttpOnly session cookie (same-origin fetch and WebSocket send it automatically).
///
/// Authenticate against the SQLite database and issue a `db-` session cookie.
///
/// Plan-02 AC-3 flow (after the per-IP `tower_governor` layer has cleared the
/// request):
/// 1. Resolve the username to a `user_id`. Unknown username → generic **401**
///    without recording any attempt (G/W/T 3.9 — locking a non-existent user
///    would leak account existence via the 429 boundary).
/// 2. Check `failed_count_in_window(user_id, password, 600) >= 5` → **429**
///    with `Retry-After` and a JSON body containing the remaining window
///    minutes (G/W/T 3.5).
/// 3. Verify the password. Failure → record an attempt with `success=0` and
///    return **401**.
/// 4. On success → record `success=1`, **clear failed counters** so the next
///    failed attempt restarts from 1 (G/W/T 3.6), apply session rotation
///    (G/W/T 5.1), then issue the session cookie.
pub async fn login(
    State(auth): State<AuthState>,
    State(config): State<ConfigState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(body): Json<LoginBody>,
) -> impl IntoResponse {
    let Some(ref db) = auth.db else {
        return (StatusCode::SERVICE_UNAVAILABLE, "Database not available").into_response();
    };

    let db_clone = db.clone();
    let has_users = tokio::task::spawn_blocking(move || {
        let conn = db_clone.conn().blocking_lock();
        maestro_core::db::users::count_users(&conn).unwrap_or(0) > 0
    })
    .await
    .unwrap_or(false);

    if !has_users {
        return (
            StatusCode::CONFLICT,
            "No users exist yet. Complete first-user setup via /api/auth/register.",
        )
            .into_response();
    }

    // Step 1: look up the user. Unknown username → 401 with NO attempt row.
    let db_clone = db.clone();
    let username = body.username.clone();
    let user_lookup = tokio::task::spawn_blocking(move || {
        let conn = db_clone.conn().blocking_lock();
        maestro_core::db::users::get_user_by_username(&conn, &username).ok().flatten()
    })
    .await
    .ok()
    .flatten();
    let Some(user) = user_lookup else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if user.suspended {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    // Step 2: lockout check.
    let db_clone = db.clone();
    let uid = user.id.clone();
    let lockout = tokio::task::spawn_blocking(move || {
        let conn = db_clone.conn().blocking_lock();
        let count = failed_count_in_window(&conn, &uid, AttemptKind::Password, LOCKOUT_WINDOW_SECS)
            .unwrap_or(0);
        if count >= LOCKOUT_THRESHOLD {
            let oldest = oldest_failure_ts_in_window(
                &conn,
                &uid,
                AttemptKind::Password,
                LOCKOUT_WINDOW_SECS,
            )
            .unwrap_or(None);
            Some((count, oldest))
        } else {
            None
        }
    })
    .await
    .ok()
    .flatten();
    if let Some((count, oldest)) = lockout {
        let now = now_unix();
        let retry_secs = oldest
            .map(|t| (t + LOCKOUT_WINDOW_SECS - now).max(60))
            .unwrap_or(60);
        let minutes = (retry_secs + 59) / 60;
        tracing::warn!(
            event = "login_lockout",
            user_id = %user.id,
            failed_count = count,
            "account temporarily locked"
        );
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [("Retry-After", retry_secs.to_string())],
            Json(serde_json::json!({
                "error": format!("account temporarily locked, try again in {minutes} minutes")
            })),
        )
            .into_response();
    }

    // Step 3: verify password (which also bumps `last_used_at` and rehashes
    // weak Argon2 hashes via Dev C's path).
    let verified = authenticate_db_user(db, &body.username, &body.password).await;

    // Step 4: record the outcome.
    let db_clone = db.clone();
    let uid = user.id.clone();
    let success = verified.is_some();
    let _ = tokio::task::spawn_blocking(move || {
        let conn = db_clone.conn().blocking_lock();
        let _ = record_attempt(&conn, &uid, AttemptKind::Password, success);
        if success {
            // Clear the failed counter so the next miss restarts at 1.
            let _ = clear_failed_attempts(&conn, &uid, AttemptKind::Password);
        }
    })
    .await;

    if !success {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    // Step 5: session rotation + create session.
    let kick = config.config.read().await.web.kick_other_sessions_on_login;
    let db_clone = db.clone();
    let user_id = user.id.clone();
    let token = tokio::task::spawn_blocking(move || {
        let conn = db_clone.conn().blocking_lock();
        if kick {
            // Plan-02 AC-5 G/W/T 5.1: delete prior sessions so the new login
            // is the only authenticated client for this user.
            let _ = maestro_core::db::credentials::delete_user_sessions(&conn, &user_id);
        }
        create_db_session(&conn, &user_id)
    })
    .await;

    let token = match token {
        Ok(Ok(t)) => t,
        _ => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    let secure = {
        let cfg = config.config.read().await;
        resolve_cookie_secure(&cfg.web, &headers)
    };

    let cookie = Cookie::build((SESSION_COOKIE_NAME, token))
        .path("/")
        .http_only(true)
        .secure(secure)
        .same_site(SameSite::Lax)
        .max_age(Duration::seconds(SESSION_IDLE_TTL_SECS as i64))
        .build();

    (jar.add(cookie), StatusCode::NO_CONTENT).into_response()
}

pub async fn logout(
    State(auth): State<AuthState>,
    State(config): State<ConfigState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> impl IntoResponse {
    // If this is a DB session, delete the server-side session record.
    if let Some(ref db) = auth.db {
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

    // The removal cookie must carry the same `Secure` resolution as the
    // session cookie it replaces — some browsers refuse to overwrite a
    // `Secure` cookie with a non-`Secure` one.
    let secure = {
        let cfg = config.config.read().await;
        resolve_cookie_secure(&cfg.web, &headers)
    };

    let mut c = Cookie::build((SESSION_COOKIE_NAME, ""))
        .path("/")
        .secure(secure)
        .max_age(Duration::seconds(0))
        .build();
    c.make_removal();
    (jar.add(c), StatusCode::NO_CONTENT).into_response()
}
