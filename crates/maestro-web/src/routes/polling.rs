// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Plan-10: auto-polling is disabled in this build (decision #4).
//!
//! The global Jira/GitHub polling model is incompatible with per-user-per-repo
//! repositories — re-enabling it is plan-11's job (per-repo polling). The
//! `GET /api/polling` and `POST /api/polling/{pause,resume}` endpoints stay
//! mounted but always report `paused: true` with an explicit reason string so
//! the dashboard polling-status surface matches reality and operators are not
//! left guessing why no tickets are picked up.

use axum::extract::{Extension, State};
use axum::http::StatusCode;
use serde::Serialize;

use crate::auth::AuthenticatedUser;
use crate::routes::admin::require_admin_for;
use crate::state::AppState;

/// Plan-10 no-op polling-status payload.
///
/// `paused` is always `true`. `reason` is a fixed string so the UI can surface
/// it verbatim without translating from an opaque code.
#[derive(Serialize)]
pub struct PollingStatus {
    pub paused: bool,
    pub reason: &'static str,
}

const DISABLED_REASON: &str = "auto-polling disabled in plan-10";

fn disabled() -> PollingStatus {
    PollingStatus {
        paused: true,
        reason: DISABLED_REASON,
    }
}

pub async fn get_polling_status(State(_state): State<AppState>) -> axum::Json<PollingStatus> {
    axum::Json(disabled())
}

pub async fn pause_polling(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<axum::Json<PollingStatus>, StatusCode> {
    // Admin gate stays so the dashboard's pause/resume controls don't surprise
    // non-admins with a 200, but the body is a no-op.
    require_admin_for(&state, &auth).await?;
    Ok(axum::Json(disabled()))
}

pub async fn resume_polling(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<axum::Json<PollingStatus>, StatusCode> {
    require_admin_for(&state, &auth).await?;
    Ok(axum::Json(disabled()))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::server::build_router;
    use crate::test_helpers::{TEST_ORIGIN, register_and_login, test_state_with_db};

    #[tokio::test]
    async fn get_polling_returns_disabled_payload() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::get("/api/polling")
                    .header("Cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["paused"], true);
        assert_eq!(json["reason"], "auto-polling disabled in plan-10");
    }

    #[tokio::test]
    async fn pause_polling_remains_no_op_with_admin_gate() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::post("/api/polling/pause")
                    .header("Origin", TEST_ORIGIN)
                    .header("Cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["paused"], true);
        assert_eq!(json["reason"], "auto-polling disabled in plan-10");
    }

    #[tokio::test]
    async fn resume_polling_remains_no_op_with_admin_gate() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::post("/api/polling/resume")
                    .header("Origin", TEST_ORIGIN)
                    .header("Cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["paused"], true);
        assert_eq!(json["reason"], "auto-polling disabled in plan-10");
    }
}
