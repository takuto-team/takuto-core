// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! `POST /api/auth/change-password`, `POST /api/auth/regenerate-recovery-codes`,
//! and `POST /api/auth/recover` — all flows that mutate password / recovery
//! credentials for an existing user.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum_extra::extract::cookie::{CookieJar, SameSite};
use cookie::Cookie;
use cookie::time::Duration;
use serde::Deserialize;

use maestro_core::auth::AuthError;

use crate::auth::{
    SESSION_COOKIE_NAME, SESSION_IDLE_TTL_SECS, create_db_session, now_unix, resolve_cookie_secure,
};
use crate::state::{AuthState, ConfigState};
use maestro_core::db::login_attempts::{
    AttemptKind, clear_failed_attempts, failed_count_in_window, oldest_failure_ts_in_window,
    record_attempt,
};

use super::{LOCKOUT_THRESHOLD, LOCKOUT_WINDOW_SECS};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangePasswordBody {
    pub current_password: String,
    pub new_password: String,
}

/// Change the current user's password. Requires valid session and correct current password.
/// Invalidates all other sessions after the change.
pub async fn change_password(
    State(auth): State<AuthState>,
    State(config): State<ConfigState>,
    headers: axum::http::HeaderMap,
    jar: CookieJar,
    Json(body): Json<ChangePasswordBody>,
) -> impl IntoResponse {
    use crate::auth::{session_cookie_from_headers, validate_db_session};

    let Some(ref db) = auth.db else {
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
            return Err(AuthError::CurrentPasswordIncorrect.into());
        }

        // Change password.
        maestro_core::db::credentials::change_password(&conn, &uid, &new_pw)?;

        // Invalidate all sessions, then re-create the current one so the user stays logged in.
        maestro_core::db::credentials::delete_user_sessions(&conn, &uid)?;
        let new_token = create_db_session(&conn, &uid)?;

        // We need to return both the old cookie (to know what to replace) and the new token.
        Ok::<_, maestro_core::error::MaestroError>((new_token, current_cookie))
    })
    .await;

    match result {
        Ok(Ok((new_token, _))) => {
            let secure = {
                let cfg = config.config.read().await;
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

/// Regenerate recovery codes for the current user. Replaces all existing codes.
/// Returns the new plaintext codes (shown once).
pub async fn regenerate_recovery_codes(
    State(auth): State<AuthState>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    use crate::auth::{session_cookie_from_headers, validate_db_session};

    let Some(ref db) = auth.db else {
        return StatusCode::BAD_REQUEST.into_response();
    };

    let cookie = session_cookie_from_headers(&headers)
        .unwrap_or_default()
        .to_string();
    let db = db.clone();
    let result = tokio::task::spawn_blocking(move || {
        let conn = db.conn().blocking_lock();
        let user_id = validate_db_session(&conn, &cookie).ok_or_else(|| -> maestro_core::error::MaestroError {
            AuthError::InvalidSession.into()
        })?;
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
    State(auth): State<AuthState>,
    Json(body): Json<RecoverBody>,
) -> impl IntoResponse {
    let Some(ref db) = auth.db else {
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
    //
    // Plan-11 step 3: login_attempts moved to the agnostic DbAdapter API.
    // Async DAO; no spawn_blocking wrapper needed.
    let adapter = db.adapter();
    let count = failed_count_in_window(adapter, &user.id, AttemptKind::Recovery, LOCKOUT_WINDOW_SECS)
        .await
        .unwrap_or(0);
    let lockout = if count >= LOCKOUT_THRESHOLD {
        let oldest = oldest_failure_ts_in_window(
            adapter,
            &user.id,
            AttemptKind::Recovery,
            LOCKOUT_WINDOW_SECS,
        )
        .await
        .unwrap_or(None);
        Some((count, oldest))
    } else {
        None
    };
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

    // Plan-11 step 3: split the inner work so login_attempts audits run
    // via the async adapter while the credentials DAO stays on the
    // legacy rusqlite path (it will migrate in a later step). The
    // recovery-code verification + password change happens inside a
    // single `spawn_blocking` block so the credentials calls share one
    // MutexGuard (avoids re-acquiring across the verify → change boundary);
    // the audit + clear are then issued from the async outer scope.
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
            // Outer scope handles the failure audit + 401.
            return Ok::<_, maestro_core::error::MaestroError>(false);
        }

        // Change password and invalidate sessions.
        maestro_core::db::credentials::change_password(&conn, &uid, &new_password)?;
        maestro_core::db::credentials::delete_user_sessions(&conn, &uid)?;

        Ok(true)
    })
    .await;

    let adapter = db.adapter();
    match result {
        Ok(Ok(true)) => {
            // Success: record audit + clear the failed counter so a
            // previously-locked-out user comes back to a fresh slate.
            let _ = record_attempt(adapter, &user.id, AttemptKind::Recovery, true).await;
            let _ = clear_failed_attempts(adapter, &user.id, AttemptKind::Recovery).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(Ok(false)) => {
            // Invalid recovery code: record the failure so the lockout
            // counter sees it on the next attempt, then return 401.
            let _ = record_attempt(adapter, &user.id, AttemptKind::Recovery, false).await;
            let msg = AuthError::InvalidRecoveryCode.to_string();
            (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": msg})),
            )
                .into_response()
        }
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
