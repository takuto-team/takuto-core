// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum_extra::extract::cookie::{CookieJar, SameSite};
use cookie::Cookie;
use cookie::time::Duration;
use serde::{Deserialize, Serialize};

use crate::auth::{
    SESSION_COOKIE_NAME, SESSION_IDLE_TTL_SECS, authenticate_db_user, create_db_session,
    delete_db_session, now_unix, resolve_cookie_secure,
};
use crate::state::AppState;
use maestro_core::db::login_attempts::{
    AttemptKind, clear_failed_attempts, failed_count_in_window, oldest_failure_ts_in_window,
    record_attempt,
};

/// Plan-02 AC-3: per-user lockout threshold and window.
///
/// 5 failed attempts within a 10-minute window locks the account until the
/// **oldest** failure ages out (sliding window — admins can short-circuit via
/// `POST /api/admin/users/{id}/unlock`).
const LOCKOUT_THRESHOLD: i64 = 5;
const LOCKOUT_WINDOW_SECS: i64 = 600;

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
    /// Phase 0 (04_architecture.md §1.3): mirror of
    /// `system_status.provider.selected` so the login page can render the
    /// right provider-specific hint without a second round-trip.
    pub provider_selected: String,
    /// Phase 0: mirror of `system_status.github.mode`.
    pub github_mode: String,
    /// Phase 0: `true` when any critical warning exists in `system_status`.
    /// The dashboard uses this to render the degraded-mode banner.
    pub degraded: bool,
}

/// Public probe: whether the server requires dashboard login.
pub async fn auth_status(State(state): State<AppState>) -> Json<AuthStatus> {
    // Phase 1: system_status is mutable (refreshed after PUT /api/config/agent),
    // so take a snapshot under the read lock and drop it before any other awaits.
    let (provider_selected, github_mode, degraded) = {
        let s = state.system_status.read().await;
        (
            s.provider.selected.clone(),
            s.github.mode.clone(),
            s.has_critical(),
        )
    };

    if let Some(ref db) = state.db {
        let db = db.clone();
        let count = tokio::task::spawn_blocking(move || {
            let conn = db.conn().blocking_lock();
            maestro_core::db::users::count_users(&conn).unwrap_or(0)
        })
        .await
        .unwrap_or(0);
        return Json(AuthStatus {
            dashboard_auth_enabled: true,
            multi_user: count > 0,
            setup_required: count == 0,
            provider_selected,
            github_mode,
            degraded,
        });
    }

    // No database — auth is required but setup cannot proceed.
    // The UI will show setup_required and the user must fix the data directory.
    Json(AuthStatus {
        dashboard_auth_enabled: true,
        multi_user: false,
        setup_required: true,
        provider_selected,
        github_mode,
        degraded,
    })
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
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
    Json(body): Json<LoginBody>,
) -> impl IntoResponse {
    let Some(ref db) = state.db else {
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
    let kick = state.config.read().await.web.kick_other_sessions_on_login;
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
        let cfg = state.config.read().await;
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
    State(state): State<AppState>,
    headers: HeaderMap,
    jar: CookieJar,
) -> impl IntoResponse {
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

    // The removal cookie must carry the same `Secure` resolution as the
    // session cookie it replaces — some browsers refuse to overwrite a
    // `Secure` cookie with a non-`Secure` one.
    let secure = {
        let cfg = state.config.read().await;
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

// ---------------------------------------------------------------------------
// Current user info
// ---------------------------------------------------------------------------

/// Returns the currently authenticated user's profile.
///
/// This endpoint is behind the auth middleware so it only succeeds when
/// a valid session cookie is present.
pub async fn me(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    use crate::auth::{session_cookie_from_headers, validate_db_session};

    let Some(ref db) = state.db else {
        // Legacy auth — no user model, return a synthetic admin.
        return Json(serde_json::json!({
            "username": state.config.read().await.web.dashboard_username.trim(),
            "role": "admin",
        }))
        .into_response();
    };

    let cookie = session_cookie_from_headers(&headers).unwrap_or_default().to_string();
    let db = db.clone();
    let user = tokio::task::spawn_blocking(move || {
        let conn = db.conn().blocking_lock();
        let user_id = validate_db_session(&conn, &cookie)?;
        maestro_core::db::users::get_user_by_id(&conn, &user_id).ok()?
    })
    .await
    .ok()
    .flatten();

    match user {
        Some(u) => Json(serde_json::json!({
            "id": u.id,
            "username": u.username,
            "role": u.role,
            "suspended": u.suspended,
        }))
        .into_response(),
        None => StatusCode::UNAUTHORIZED.into_response(),
    }
}

// ---------------------------------------------------------------------------
// Password change
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangePasswordBody {
    pub current_password: String,
    pub new_password: String,
}

/// Change the current user's password. Requires valid session and correct current password.
/// Invalidates all other sessions after the change.
pub async fn change_password(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    jar: CookieJar,
    Json(body): Json<ChangePasswordBody>,
) -> impl IntoResponse {
    use crate::auth::{session_cookie_from_headers, validate_db_session};

    let Some(ref db) = state.db else {
        return StatusCode::BAD_REQUEST.into_response();
    };

    // Resolve the current user from the session cookie.
    let cookie = session_cookie_from_headers(&headers)
        .unwrap_or_default()
        .to_string();
    let db_clone = db.clone();
    let user_id = tokio::task::spawn_blocking(move || {
        let conn = db_clone.conn().blocking_lock();
        validate_db_session(&conn, &cookie)
    })
    .await
    .ok()
    .flatten();

    let Some(user_id) = user_id else {
        return StatusCode::UNAUTHORIZED.into_response();
    };

    if body.new_password.len() < 12 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "New password must be at least 12 characters"})),
        )
            .into_response();
    }

    let db_clone = db.clone();
    let current_pw = body.current_password;
    let new_pw = body.new_password;
    let uid = user_id.clone();
    let current_cookie = jar
        .get(SESSION_COOKIE_NAME)
        .map(|c| c.value().to_string())
        .unwrap_or_default();

    let result = tokio::task::spawn_blocking(move || {
        let conn = db_clone.conn().blocking_lock();

        // Verify current password.
        if !maestro_core::db::credentials::verify_user_password(&conn, &uid, &current_pw)? {
            return Err(maestro_core::error::MaestroError::Auth(
                "Current password is incorrect".into(),
            ));
        }

        // Change password.
        maestro_core::db::credentials::change_password(&conn, &uid, &new_pw)?;

        // Invalidate all sessions, then re-create the current one so the user stays logged in.
        maestro_core::db::credentials::delete_user_sessions(&conn, &uid)?;
        let new_token = create_db_session(&conn, &uid)?;

        // We need to return both the old cookie (to know what to replace) and the new token.
        Ok((new_token, current_cookie))
    })
    .await;

    match result {
        Ok(Ok((new_token, _))) => {
            let secure = {
                let cfg = state.config.read().await;
                resolve_cookie_secure(&cfg.web, &headers)
            };
            let cookie = Cookie::build((SESSION_COOKIE_NAME, new_token))
                .path("/")
                .http_only(true)
                .secure(secure)
                .same_site(SameSite::Lax)
                .max_age(Duration::seconds(SESSION_IDLE_TTL_SECS as i64))
                .build();
            (jar.add(cookie), StatusCode::NO_CONTENT).into_response()
        }
        Ok(Err(e)) => {
            let msg = e.to_string();
            if msg.contains("incorrect") {
                (
                    StatusCode::UNAUTHORIZED,
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

// ---------------------------------------------------------------------------
// Regenerate recovery codes
// ---------------------------------------------------------------------------

/// Regenerate recovery codes for the current user. Replaces all existing codes.
/// Returns the new plaintext codes (shown once).
pub async fn regenerate_recovery_codes(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    use crate::auth::{session_cookie_from_headers, validate_db_session};

    let Some(ref db) = state.db else {
        return StatusCode::BAD_REQUEST.into_response();
    };

    let cookie = session_cookie_from_headers(&headers)
        .unwrap_or_default()
        .to_string();
    let db = db.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = db.conn().blocking_lock();
        let user_id = validate_db_session(&conn, &cookie)
            .ok_or_else(|| maestro_core::error::MaestroError::Auth("Invalid session".into()))?;
        let codes =
            maestro_core::db::credentials::generate_recovery_codes(&conn, &user_id, 8)?;
        Ok::<_, maestro_core::error::MaestroError>(codes)
    })
    .await;

    match result {
        Ok(Ok(codes)) => Json(serde_json::json!({ "recovery_codes": codes })).into_response(),
        Ok(Err(e)) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

// ---------------------------------------------------------------------------
// Account recovery via recovery code
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoverBody {
    pub username: String,
    pub recovery_code: String,
    pub new_password: String,
}

/// Reset a user's password using a single-use recovery code.
///
/// This is a **public** endpoint (no session required — the user is locked out).
/// On success, the recovery code is consumed, the password is changed, and all
/// existing sessions are invalidated.
pub async fn recover(
    State(state): State<AppState>,
    Json(body): Json<RecoverBody>,
) -> impl IntoResponse {
    let Some(ref db) = state.db else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Database not available"})),
        )
            .into_response();
    };

    if body.new_password.len() < 12 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "New password must be at least 12 characters"})),
        )
            .into_response();
    }

    // Plan-02 AC-3 G/W/T 3.7: separate per-user counter for recovery attempts.
    // The lockout threshold and window match the password path, but the counter
    // is keyed by `AttemptKind::Recovery` so a brute-force on recovery codes
    // doesn't slip past the password counter and vice versa.
    let db_clone = db.clone();
    let username = body.username.clone();
    let user_lookup = tokio::task::spawn_blocking(move || {
        let conn = db_clone.conn().blocking_lock();
        maestro_core::db::users::get_user_by_username(&conn, &username).ok().flatten()
    })
    .await
    .ok()
    .flatten();
    // G/W/T 3.9 equivalent for recovery: unknown user → generic 401 without
    // recording an attempt (lockout DoS would otherwise be free for any attacker
    // who can guess a username pattern).
    let Some(user) = user_lookup else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "Invalid recovery code"})),
        )
            .into_response();
    };
    if user.suspended {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "Account is suspended"})),
        )
            .into_response();
    }

    // Lockout check for the recovery counter.
    let db_clone = db.clone();
    let uid = user.id.clone();
    let lockout = tokio::task::spawn_blocking(move || {
        let conn = db_clone.conn().blocking_lock();
        let count = failed_count_in_window(&conn, &uid, AttemptKind::Recovery, LOCKOUT_WINDOW_SECS)
            .unwrap_or(0);
        if count >= LOCKOUT_THRESHOLD {
            let oldest = oldest_failure_ts_in_window(
                &conn,
                &uid,
                AttemptKind::Recovery,
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
            kind = "recovery",
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

    let db_clone = db.clone();
    let uid = user.id.clone();
    let code = body.recovery_code;
    let new_password = body.new_password;

    let result = tokio::task::spawn_blocking(move || {
        let conn = db_clone.conn().blocking_lock();

        // Verify and consume the recovery code.
        let valid = maestro_core::db::credentials::verify_and_consume_recovery_code(
            &conn, &uid, &code,
        )?;
        if !valid {
            // Audit the failure before bubbling the error up so the lockout
            // counter sees it on the next attempt.
            let _ = record_attempt(&conn, &uid, AttemptKind::Recovery, false);
            return Err(maestro_core::error::MaestroError::Auth(
                "Invalid recovery code".into(),
            ));
        }

        // Record success and clear the failed counter so a future locked-out
        // user comes back to a fresh slate after a successful recovery.
        let _ = record_attempt(&conn, &uid, AttemptKind::Recovery, true);
        let _ = clear_failed_attempts(&conn, &uid, AttemptKind::Recovery);

        // Change password and invalidate sessions.
        maestro_core::db::credentials::change_password(&conn, &uid, &new_password)?;
        maestro_core::db::credentials::delete_user_sessions(&conn, &uid)?;

        Ok(())
    })
    .await;

    match result {
        Ok(Ok(())) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(e)) => {
            let msg = e.to_string();
            (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": msg})),
            )
                .into_response()
        }
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "Internal server error"})),
        )
            .into_response(),
    }
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
///
/// Phase 1 (auth-overhaul, 06_qa_and_blind_spots.md §A.3 T-ONB-001): the
/// just-created admin must land on the 4-step onboarding wizard, not the empty
/// dashboard. The server advertises that next-hop in `redirect_to` so the UI
/// (and any non-browser API consumers) don't have to hard-code the path.
#[derive(Debug, Serialize)]
struct RegisterResponse {
    user_id: String,
    username: String,
    role: String,
    recovery_codes: Vec<String>,
    /// Always `"/onboarding"` on first-user setup success.
    redirect_to: &'static str,
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
        if password.len() < 12 {
            return Err(maestro_core::error::MaestroError::Auth(
                "Password must be at least 12 characters".into(),
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
        let codes = maestro_core::db::credentials::generate_recovery_codes(&conn, &user.id, 8)?;

        Ok(RegisterResponse {
            user_id: user.id,
            username: user.username,
            role: user.role.as_str().to_string(),
            recovery_codes: codes,
            redirect_to: "/onboarding",
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
