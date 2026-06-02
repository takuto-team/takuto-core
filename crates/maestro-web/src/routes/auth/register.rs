// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! `POST /api/auth/register` — first-user setup.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

use maestro_core::auth::AuthError;

use crate::state::AuthState;

/// Request body for first-user registration.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterBody {
    pub username: String,
    pub password: String,
}

/// Registration response containing recovery codes.
///
/// The just-created admin must land on the 4-step onboarding wizard, not
/// the empty dashboard. The server advertises that next-hop in
/// `redirect_to` so the UI (and any non-browser API consumers) don't have
/// to hard-code the path.
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
/// `auth.db` is `Some` and the users table is empty.
pub async fn register(
    State(auth): State<AuthState>,
    Json(body): Json<RegisterBody>,
) -> impl IntoResponse {
    let Some(ref db) = auth.db else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Database not available"})),
        )
            .into_response();
    };

    // The credential writes (store_password + generate_recovery_codes)
    // co-commit via one transaction; create_user opens its own internal
    // tx with the first-user-becomes-admin race guard.
    let adapter = db.adapter();
    let username = body.username;
    let password = body.password;

    let result: maestro_core::error::Result<RegisterResponse> = async {
        let count = maestro_core::db::users::count_users(adapter).await?;
        if count > 0 {
            return Err(AuthError::RegistrationClosed.into());
        }
        if username.trim().is_empty() {
            return Err(AuthError::EmptyUsername.into());
        }
        if password.len() < 12 {
            return Err(AuthError::PasswordTooShort.into());
        }
        let user = maestro_core::db::users::create_user(
            adapter,
            &username,
            maestro_core::db::models::UserRole::Admin,
        )
        .await?;
        let mut tx = adapter.begin().await?;
        maestro_core::db::credentials::store_password(&mut tx, &user.id, &password).await?;
        let codes = maestro_core::db::credentials::generate_recovery_codes(&mut tx, &user.id, 8)
            .await?;
        tx.commit().await?;
        Ok(RegisterResponse {
            user_id: user.id,
            username: user.username,
            role: user.role.as_str().to_string(),
            recovery_codes: codes,
            redirect_to: "/onboarding",
        })
    }
    .await;

    match result {
        Ok(resp) => (StatusCode::CREATED, Json(serde_json::json!(resp))).into_response(),
        Err(e) => {
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
    }
}
