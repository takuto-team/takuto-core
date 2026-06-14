// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! `GET /api/auth/me` — current user info.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;

use crate::state::AuthState;

/// Returns the currently authenticated user's profile.
///
/// This endpoint is behind the auth middleware so it only succeeds when
/// a valid session cookie is present.
pub async fn me(
    State(auth): State<AuthState>,
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    use crate::auth::{session_cookie_from_headers, validate_db_session};

    let Some(ref db) = auth.db else {
        // No database — the auth middleware rejects protected requests before
        // they reach this handler, so this branch is effectively unreachable.
        return StatusCode::UNAUTHORIZED.into_response();
    };

    let adapter = db.adapter();
    let cookie = session_cookie_from_headers(&headers).unwrap_or_default();
    let user_id = validate_db_session(adapter, cookie).await;
    let user = if let Some(uid) = user_id {
        maestro_core::db::users::get_user_by_id(adapter, &uid)
            .await
            .ok()
            .flatten()
    } else {
        None
    };

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
