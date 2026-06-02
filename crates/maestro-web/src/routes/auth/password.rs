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
    let cookie = session_cookie_from_headers(&headers).unwrap_or_default();
    let user_id = validate_db_session(db.adapter(), cookie).await;

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

    let current_pw = body.current_password;
    let new_pw = body.new_password;
    let uid = user_id.clone();
    let current_cookie = jar
        .get(SESSION_COOKIE_NAME)
        .map(|c| c.value().to_string())
        .unwrap_or_default();

    // The transaction covers the credential rotation; the follow-up
    // create_db_session is a separate adapter call.
    let adapter = db.adapter();
    let result: maestro_core::error::Result<(String, String)> = async {
        if !maestro_core::db::credentials::verify_user_password(adapter, &uid, &current_pw).await? {
            return Err(AuthError::CurrentPasswordIncorrect.into());
        }
        let mut tx = adapter.begin().await?;
        maestro_core::db::credentials::change_password(&mut tx, &uid, &new_pw).await?;
        maestro_core::db::credentials::delete_user_sessions(&mut tx, &uid).await?;
        tx.commit().await?;
        let new_token = create_db_session(adapter, &uid).await?;
        Ok((new_token, current_cookie.clone()))
    }
    .await;

    match result {
        Ok((new_token, _)) => {
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
        Err(e) => {
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

    let cookie = session_cookie_from_headers(&headers).unwrap_or_default();
    let user_id = validate_db_session(db.adapter(), cookie).await;
    let Some(user_id) = user_id else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "Invalid session"})),
        )
            .into_response();
    };
    let adapter = db.adapter();
    let result: maestro_core::error::Result<Vec<String>> = async {
        let mut tx = adapter.begin().await?;
        let codes =
            maestro_core::db::credentials::generate_recovery_codes(&mut tx, &user_id, 8).await?;
        tx.commit().await?;
        Ok(codes)
    }
    .await;

    match result {
        Ok(codes) => Json(serde_json::json!({ "recovery_codes": codes })).into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
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

    // Separate per-user counter for recovery attempts. The lockout
    // threshold and window match the password path, but the counter is
    // keyed by `AttemptKind::Recovery` so a brute-force on recovery codes
    // doesn't slip past the password counter and vice versa.
    let user_lookup = maestro_core::db::users::get_user_by_username(db.adapter(), &body.username)
        .await
        .ok()
        .flatten();
    // Unknown user → generic 401 without recording an attempt (lockout
    // DoS would otherwise be free for any attacker who can guess a
    // username pattern).
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
    let adapter = db.adapter();
    let count = failed_count_in_window(
        adapter,
        &user.id,
        AttemptKind::Recovery,
        LOCKOUT_WINDOW_SECS,
    )
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

    // The recover-flow's verify_and_consume + change_password +
    // delete_user_sessions all run in one DbTransaction so they commit
    // atomically.
    let adapter = db.adapter();
    let uid = user.id.clone();
    let code = body.recovery_code;
    let new_password = body.new_password;

    let result: maestro_core::error::Result<bool> = async {
        let mut tx = adapter.begin().await?;
        let valid =
            maestro_core::db::credentials::verify_and_consume_recovery_code(&mut tx, &uid, &code)
                .await?;
        if !valid {
            // Outer scope handles the failure audit + 401. Roll back the
            // transaction — even though no rows changed (the consume
            // only runs on match), being explicit keeps semantics clean.
            tx.rollback().await?;
            return Ok(false);
        }
        maestro_core::db::credentials::change_password(&mut tx, &uid, &new_password).await?;
        maestro_core::db::credentials::delete_user_sessions(&mut tx, &uid).await?;
        tx.commit().await?;
        Ok(true)
    }
    .await;

    match result {
        Ok(true) => {
            let _ = record_attempt(adapter, &user.id, AttemptKind::Recovery, true).await;
            let _ = clear_failed_attempts(adapter, &user.id, AttemptKind::Recovery).await;
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => {
            let _ = record_attempt(adapter, &user.id, AttemptKind::Recovery, false).await;
            let msg = AuthError::InvalidRecoveryCode.to_string();
            (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": msg})),
            )
                .into_response()
        }
        Err(e) => {
            let msg = e.to_string();
            (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": msg})),
            )
                .into_response()
        }
    }
}
