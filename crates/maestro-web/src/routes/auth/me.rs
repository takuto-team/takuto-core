// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! `GET /api/auth/me` — current user info.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;

use crate::state::{AuthState, ConfigState};

/// Returns the currently authenticated user's profile.
///
/// This endpoint is behind the auth middleware so it only succeeds when
/// a valid session cookie is present.
pub async fn me(
    State(auth): State<AuthState>,
    State(config): State<ConfigState>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    use crate::auth::{session_cookie_from_headers, validate_db_session};

    let Some(ref db) = auth.db else {
        // Legacy auth — no user model, return a synthetic admin.
        return Json(serde_json::json!({
            "username": config.config.read().await.web.dashboard_username.trim(),
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
