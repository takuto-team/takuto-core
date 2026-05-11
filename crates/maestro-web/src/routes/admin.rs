// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Admin user management routes.
//!
//! All handlers require a valid database session cookie belonging to a user with
//! the `admin` role. Returns `403 FORBIDDEN` for non-admin users.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use serde::Serialize;

use maestro_core::db::models::{
    CreateUserRequest, ImportSummary, SkippedUser, UpdateUserRequest, User, UserExport, UserRole,
};

use crate::auth::{session_cookie_from_headers, validate_db_session};
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Admin authorisation helper
// ---------------------------------------------------------------------------

/// Extract and validate the session cookie, then verify the user has the admin role.
///
/// Returns the authenticated admin [`User`] on success, or the appropriate HTTP
/// status code on failure.
async fn require_admin(state: &AppState, headers: &HeaderMap) -> Result<User, StatusCode> {
    let db = state.db.as_ref().ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;

    let raw_cookie = session_cookie_from_headers(headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let cookie = raw_cookie.to_string();
    let db_clone = db.clone();

    let user = tokio::task::spawn_blocking(move || {
        let conn = db_clone.conn().blocking_lock();
        let user_id = validate_db_session(&conn, &cookie)?;
        maestro_core::db::users::get_user_by_id(&conn, &user_id)
            .ok()
            .flatten()
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .ok_or(StatusCode::UNAUTHORIZED)?;

    if user.role != UserRole::Admin {
        return Err(StatusCode::FORBIDDEN);
    }
    if user.suspended {
        return Err(StatusCode::FORBIDDEN);
    }

    Ok(user)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /api/users` -- List all users (admin only).
pub async fn list_users(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    if let Err(status) = require_admin(&state, &headers).await {
        return status.into_response();
    }

    let db = match state.db.as_ref() {
        Some(db) => db.clone(),
        None => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.conn().blocking_lock();
        maestro_core::db::users::list_users(&conn)
    })
    .await;

    match result {
        Ok(Ok(users)) => Json(users).into_response(),
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// Response for user creation with optional recovery codes.
#[derive(Serialize)]
struct CreateUserResponse {
    user: User,
    #[serde(skip_serializing_if = "Option::is_none")]
    recovery_codes: Option<Vec<String>>,
}

/// `POST /api/users` -- Create a new user (admin only).
pub async fn create_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateUserRequest>,
) -> impl IntoResponse {
    if let Err(status) = require_admin(&state, &headers).await {
        return status.into_response();
    }

    let db = match state.db.as_ref() {
        Some(db) => db.clone(),
        None => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.conn().blocking_lock();
        let role = body.role.unwrap_or(UserRole::User);
        let user = maestro_core::db::users::create_user(&conn, &body.username, role)?;

        let mut codes = None;
        if let Some(ref password) = body.password
            && !password.is_empty()
        {
            maestro_core::db::credentials::store_password(&conn, &user.id, password)?;
            let recovery =
                maestro_core::db::credentials::generate_recovery_codes(&conn, &user.id, 8)?;
            codes = Some(recovery);
        }

        Ok::<_, maestro_core::error::MaestroError>(CreateUserResponse {
            user,
            recovery_codes: codes,
        })
    })
    .await;

    match result {
        Ok(Ok(resp)) => (StatusCode::CREATED, Json(serde_json::json!(resp))).into_response(),
        Ok(Err(e)) => {
            let msg = e.to_string();
            let status = if msg.contains("already exists") {
                StatusCode::CONFLICT
            } else {
                StatusCode::BAD_REQUEST
            };
            (status, Json(serde_json::json!({"error": msg}))).into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// `GET /api/users/{id}` -- Get a single user (admin only).
pub async fn get_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Err(status) = require_admin(&state, &headers).await {
        return status.into_response();
    }

    let db = match state.db.as_ref() {
        Some(db) => db.clone(),
        None => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.conn().blocking_lock();
        maestro_core::db::users::get_user_by_id(&conn, &id)
    })
    .await;

    match result {
        Ok(Ok(Some(user))) => Json(user).into_response(),
        Ok(Ok(None)) => StatusCode::NOT_FOUND.into_response(),
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// `PATCH /api/users/{id}` -- Update a user (admin only).
pub async fn update_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<UpdateUserRequest>,
) -> impl IntoResponse {
    let admin = match require_admin(&state, &headers).await {
        Ok(user) => user,
        Err(status) => return status.into_response(),
    };

    // Admins cannot demote themselves (spec: admin cannot demote their own account).
    if admin.id == id
        && let Some(new_role) = body.role
        && new_role != admin.role
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Admins cannot change their own role"})),
        )
            .into_response();
    }

    let db = match state.db.as_ref() {
        Some(db) => db.clone(),
        None => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.conn().blocking_lock();
        maestro_core::db::users::update_user(&conn, &id, body.username.as_deref(), body.role)
    })
    .await;

    match result {
        Ok(Ok(user)) => Json(user).into_response(),
        Ok(Err(e)) => {
            let msg = e.to_string();
            let status = if msg.contains("not found") || msg.contains("Not found") {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::BAD_REQUEST
            };
            (status, Json(serde_json::json!({"error": msg}))).into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// `POST /api/users/{id}/suspend` -- Suspend a user (admin only).
pub async fn suspend_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let admin = match require_admin(&state, &headers).await {
        Ok(user) => user,
        Err(status) => return status.into_response(),
    };

    // Admins cannot suspend themselves (spec: admin cannot suspend their own account).
    if admin.id == id {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Admins cannot suspend their own account"})),
        )
            .into_response();
    }

    let db = match state.db.as_ref() {
        Some(db) => db.clone(),
        None => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.conn().blocking_lock();
        maestro_core::db::users::suspend_user(&conn, &id)?;
        // Also delete all sessions for the suspended user.
        maestro_core::db::credentials::delete_user_sessions(&conn, &id)?;
        Ok::<_, maestro_core::error::MaestroError>(())
    })
    .await;

    match result {
        Ok(Ok(())) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(e)) => {
            let msg = e.to_string();
            let status = if msg.contains("not found") || msg.contains("Not found") {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::BAD_REQUEST
            };
            (status, Json(serde_json::json!({"error": msg}))).into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// `POST /api/users/{id}/unsuspend` -- Unsuspend a user (admin only).
pub async fn unsuspend_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Err(status) = require_admin(&state, &headers).await {
        return status.into_response();
    }

    let db = match state.db.as_ref() {
        Some(db) => db.clone(),
        None => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.conn().blocking_lock();
        maestro_core::db::users::unsuspend_user(&conn, &id)
    })
    .await;

    match result {
        Ok(Ok(())) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(e)) => {
            let msg = e.to_string();
            let status = if msg.contains("not found") || msg.contains("Not found") {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::BAD_REQUEST
            };
            (status, Json(serde_json::json!({"error": msg}))).into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// `DELETE /api/users/{id}` -- Delete a user and all associated data (admin only).
pub async fn delete_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let admin = match require_admin(&state, &headers).await {
        Ok(user) => user,
        Err(status) => return status.into_response(),
    };

    // Admins cannot delete themselves (spec: admin cannot delete their own account).
    if admin.id == id {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "Admins cannot delete their own account"})),
        )
            .into_response();
    }

    let db = match state.db.as_ref() {
        Some(db) => db.clone(),
        None => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.conn().blocking_lock();
        maestro_core::db::users::delete_user(&conn, &id)
    })
    .await;

    match result {
        Ok(Ok(())) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(e)) => {
            let msg = e.to_string();
            let status = if msg.contains("not found") || msg.contains("Not found") {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::BAD_REQUEST
            };
            (status, Json(serde_json::json!({"error": msg}))).into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// `GET /api/users/export` -- Export all users (admin only).
pub async fn export_users(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    if let Err(status) = require_admin(&state, &headers).await {
        return status.into_response();
    }

    let db = match state.db.as_ref() {
        Some(db) => db.clone(),
        None => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.conn().blocking_lock();
        let users = maestro_core::db::users::list_users(&conn)?;
        let exports: Vec<UserExport> = users
            .into_iter()
            .map(|u| UserExport {
                username: u.username,
                role: u.role,
                suspended: u.suspended,
                created_at: u.created_at,
            })
            .collect();
        Ok::<_, maestro_core::error::MaestroError>(exports)
    })
    .await;

    match result {
        Ok(Ok(exports)) => Json(exports).into_response(),
        Ok(Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// `POST /api/users/import` -- Import users (admin only).
///
/// Creates users from the import list. Skips users whose username already exists.
/// Imported users have no password set (admin must set one separately).
pub async fn import_users(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Vec<UserExport>>,
) -> impl IntoResponse {
    if let Err(status) = require_admin(&state, &headers).await {
        return status.into_response();
    }

    let db = match state.db.as_ref() {
        Some(db) => db.clone(),
        None => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.conn().blocking_lock();

        // Validate all users first before writing anything.
        let mut to_create = Vec::new();
        let mut skipped = Vec::new();

        for export in &body {
            let username = export.username.trim();
            if username.is_empty() {
                skipped.push(SkippedUser {
                    username: export.username.clone(),
                    reason: "Username cannot be empty".into(),
                });
                continue;
            }
            // Check for existing username.
            match maestro_core::db::users::get_user_by_username(&conn, username) {
                Ok(Some(_)) => {
                    skipped.push(SkippedUser {
                        username: export.username.clone(),
                        reason: format!("Username '{}' already exists", username),
                    });
                }
                Ok(None) => {
                    to_create.push(export);
                }
                Err(e) => {
                    return Err(maestro_core::error::MaestroError::Database(e.to_string()));
                }
            }
        }

        // Wrap all inserts in a single transaction for atomicity.
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| maestro_core::error::MaestroError::Database(e.to_string()))?;

        let mut created = Vec::new();
        for export in to_create {
            let id = uuid::Uuid::new_v4().to_string();
            let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
            tx.execute(
                "INSERT INTO users (id, username, role, suspended, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    id,
                    export.username.trim(),
                    export.role.as_str(),
                    export.suspended as i32,
                    now,
                    now,
                ],
            )
            .map_err(|e| maestro_core::error::MaestroError::Database(e.to_string()))?;
            created.push(export.username.clone());
        }

        tx.commit()
            .map_err(|e| maestro_core::error::MaestroError::Database(e.to_string()))?;

        Ok(ImportSummary { created, skipped })
    })
    .await;

    match result {
        Ok(Ok(summary)) => (StatusCode::CREATED, Json(serde_json::json!(summary))).into_response(),
        Ok(Err(e)) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
    use http_body_util::BodyExt;
    use tokio::sync::RwLock;
    use tower::ServiceExt;

    use maestro_core::actions::dry_run::DryRunActions;
    use maestro_core::config::{Config, TicketingSystem};
    use maestro_core::db::Database;
    use maestro_core::workflow::engine::WorkflowEngine;

    use crate::server::build_router;
    use crate::state::AppState;

    /// Create a fresh Database backed by a temp directory (in-memory is `#[cfg(test)]`-gated
    /// in `maestro-core` and unavailable from downstream crate tests).
    fn temp_db() -> Database {
        let dir = std::env::temp_dir().join(format!("maestro-test-{}", uuid::Uuid::new_v4()));
        Database::open(&dir).expect("failed to create temp test database")
    }

    /// Create a test `AppState` with an in-memory SQLite database.
    fn test_state_with_db(db: Database) -> AppState {
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
            db: Some(db),
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

    /// Register the first user (admin) and return the DB session cookie value.
    async fn register_admin(state: &AppState) -> String {
        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::post("/api/auth/register")
                    .header("Content-Type", "application/json")
                    .body(Body::from(r#"{"username":"admin","password":"secret123"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::CREATED,
            "register should succeed"
        );

        // Login to get a session cookie.
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
        set_cookie.split(';').next().unwrap().trim().to_string()
    }

    /// Get the admin user's ID from the database.
    async fn get_admin_id(db: &Database) -> String {
        let db = db.clone();
        tokio::task::spawn_blocking(move || {
            let conn = db.conn().blocking_lock();
            maestro_core::db::users::get_user_by_username(&conn, "admin")
                .unwrap()
                .unwrap()
                .id
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn admin_routes_require_authentication() {
        let db = temp_db();
        let state = test_state_with_db(db);
        let app = build_router(state.clone());

        // No cookie → 401.
        let resp = app
            .oneshot(Request::get("/api/users").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn list_users_returns_registered_admin() {
        let db = temp_db();
        let state = test_state_with_db(db);
        let cookie = register_admin(&state).await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::get("/api/users")
                    .header("Cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let users = json.as_array().unwrap();
        assert_eq!(users.len(), 1);
        assert_eq!(users[0]["username"], "admin");
    }

    #[tokio::test]
    async fn admin_cannot_suspend_themselves() {
        let db = temp_db();
        let state = test_state_with_db(db.clone());
        let cookie = register_admin(&state).await;
        let admin_id = get_admin_id(&db).await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::post(&format!("/api/users/{admin_id}/suspend"))
                    .header("Cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(
            json["error"]
                .as_str()
                .unwrap()
                .contains("cannot suspend their own"),
            "expected self-suspend error, got: {json}"
        );
    }

    #[tokio::test]
    async fn admin_cannot_delete_themselves() {
        let db = temp_db();
        let state = test_state_with_db(db.clone());
        let cookie = register_admin(&state).await;
        let admin_id = get_admin_id(&db).await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri(&format!("/api/users/{admin_id}"))
                    .header("Cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(
            json["error"]
                .as_str()
                .unwrap()
                .contains("cannot delete their own"),
            "expected self-delete error, got: {json}"
        );
    }

    #[tokio::test]
    async fn admin_cannot_demote_themselves() {
        let db = temp_db();
        let state = test_state_with_db(db.clone());
        let cookie = register_admin(&state).await;
        let admin_id = get_admin_id(&db).await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::PATCH)
                    .uri(&format!("/api/users/{admin_id}"))
                    .header("Cookie", &cookie)
                    .header("Content-Type", "application/json")
                    .body(Body::from(r#"{"role":"user"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(
            json["error"]
                .as_str()
                .unwrap()
                .contains("cannot change their own role"),
            "expected self-demote error, got: {json}"
        );
    }

    #[tokio::test]
    async fn create_user_returns_201() {
        let db = temp_db();
        let state = test_state_with_db(db);
        let cookie = register_admin(&state).await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::post("/api/users")
                    .header("Cookie", &cookie)
                    .header("Content-Type", "application/json")
                    .body(Body::from(r#"{"username":"bob","password":"pass123"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn export_excludes_credentials() {
        let db = temp_db();
        let state = test_state_with_db(db);
        let cookie = register_admin(&state).await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::get("/api/users/export")
                    .header("Cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let users = json.as_array().unwrap();
        assert_eq!(users.len(), 1);
        // Export should contain username, role, suspended, created_at — no password/credential data.
        assert!(users[0].get("username").is_some());
        assert!(users[0].get("role").is_some());
        assert!(users[0].get("password").is_none());
        assert!(users[0].get("credentials").is_none());
    }

    #[tokio::test]
    async fn import_returns_201_and_skips_duplicates() {
        let db = temp_db();
        let state = test_state_with_db(db);
        let cookie = register_admin(&state).await;

        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::post("/api/users/import")
                    .header("Cookie", &cookie)
                    .header("Content-Type", "application/json")
                    .body(Body::from(
                        r#"[
                            {"username":"admin","role":"admin","suspended":false,"created_at":"2026-01-01T00:00:00Z"},
                            {"username":"charlie","role":"user","suspended":false,"created_at":"2026-01-01T00:00:00Z"}
                        ]"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let created = json["created"].as_array().unwrap();
        let skipped = json["skipped"].as_array().unwrap();
        assert_eq!(created.len(), 1, "charlie should be created");
        assert_eq!(created[0], "charlie");
        assert_eq!(skipped.len(), 1, "admin should be skipped (duplicate)");
        assert_eq!(skipped[0]["username"], "admin");
    }
}
