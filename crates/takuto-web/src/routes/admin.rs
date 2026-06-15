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

use takuto_core::auth::AuthError;
use takuto_core::db::models::{
    CreateUserRequest, ImportSummary, SkippedUser, UpdateUserRequest, User, UserExport, UserRole,
};

use crate::auth::{AuthenticatedUser, session_cookie_from_headers, validate_db_session};
use crate::state::{AuthState, ConfigState};

// ---------------------------------------------------------------------------
// Admin authorisation helpers
// ---------------------------------------------------------------------------

/// Preferred admin check for handlers downstream of the auth middleware.
///
/// The auth middleware has already validated the session cookie and inserted
/// an [`AuthenticatedUser`] into the request extensions — this helper simply
/// asserts that the role is `admin`. Returns `Ok(())` for admins and
/// `Err(StatusCode::FORBIDDEN)` otherwise.
///
/// Use [`Extension(auth): Extension<AuthenticatedUser>`] in the handler
/// signature and call this as the first line of the handler body.
pub(crate) async fn require_admin_for(
    _auth_state: &AuthState,
    auth: &AuthenticatedUser,
) -> Result<(), StatusCode> {
    if auth.role == UserRole::Admin {
        Ok(())
    } else {
        Err(StatusCode::FORBIDDEN)
    }
}

/// Legacy admin check that re-reads the session cookie from headers and
/// re-fetches the user record. Kept for back-compat with the existing
/// `routes/admin.rs` user-management handlers, which depend on the returned
/// [`User`] (e.g. for the "admin cannot demote/suspend/delete themselves"
/// guard rails). New code should prefer [`require_admin_for`].
///
/// Returns the authenticated admin [`User`] on success, or the appropriate HTTP
/// status code on failure.
pub(crate) async fn require_admin(
    auth_state: &AuthState,
    headers: &HeaderMap,
) -> Result<User, StatusCode> {
    let db = auth_state
        .db
        .as_ref()
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;

    let raw_cookie = session_cookie_from_headers(headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let adapter = db.adapter();
    let user_id = validate_db_session(adapter, raw_cookie)
        .await
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let user = takuto_core::db::users::get_user_by_id(adapter, &user_id)
        .await
        .ok()
        .flatten()
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
pub async fn list_users(State(auth): State<AuthState>, headers: HeaderMap) -> impl IntoResponse {
    if let Err(status) = require_admin(&auth, &headers).await {
        return status.into_response();
    }

    let db = match auth.db.as_ref() {
        Some(db) => db,
        None => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    match takuto_core::db::users::list_users(db.adapter()).await {
        Ok(users) => Json(users).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
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
    State(auth): State<AuthState>,
    State(config): State<ConfigState>,
    headers: HeaderMap,
    Json(body): Json<CreateUserRequest>,
) -> impl IntoResponse {
    if let Err(status) = require_admin(&auth, &headers).await {
        return status.into_response();
    }

    let db = match auth.db.as_ref() {
        Some(db) => db,
        None => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let adapter = db.adapter();

    // create_user opens its own internal tx; password + recovery codes
    // go through a single explicit tx so they co-commit.
    let role = body.role.unwrap_or(UserRole::User);
    let result: takuto_core::error::Result<CreateUserResponse> = async {
        let user = takuto_core::db::users::create_user(adapter, &body.username, role).await?;
        let mut codes = None;
        if let Some(ref password) = body.password
            && !password.is_empty()
        {
            if password.len() < 12 {
                return Err(AuthError::PasswordTooShort.into());
            }
            let mut tx = adapter.begin().await?;
            takuto_core::db::credentials::store_password(&mut tx, &user.id, password).await?;
            let recovery =
                takuto_core::db::credentials::generate_recovery_codes(&mut tx, &user.id, 8).await?;
            tx.commit().await?;
            codes = Some(recovery);
        }
        Ok(CreateUserResponse {
            user,
            recovery_codes: codes,
        })
    }
    .await;

    match result {
        Ok(resp) => {
            seed_default_flows_best_effort(&config, adapter, &resp.user.id).await;
            (StatusCode::CREATED, Json(serde_json::json!(resp))).into_response()
        }
        Err(e) => {
            let msg = e.to_string();
            let status = if msg.contains("already exists") {
                StatusCode::CONFLICT
            } else {
                StatusCode::BAD_REQUEST
            };
            (status, Json(serde_json::json!({"error": msg}))).into_response()
        }
    }
}

/// Seed the default work-item flows for a freshly-created user against the
/// active workspace. Best-effort: a failure logs a warning and leaves the
/// user's row absent, so they see the dashboard empty-state banner until they
/// save a flow or re-seed.
///
/// This is the eager seed path. A workspace this misses (e.g. one the active
/// repo switches to later) is still covered: the flow resolver lazily calls
/// `seed_if_absent` and retries once when the DAO returns no row, so there is
/// no need for a separate seed-on-workspace-switch hook.
pub(crate) async fn seed_default_flows_best_effort(
    config: &ConfigState,
    adapter: &takuto_core::db::DbAdapter,
    user_id: &str,
) {
    let workspace = config.active_workspace_name().await;
    if let Err(e) = takuto_core::db::user_work_item_flows::seed_if_absent(
        adapter,
        user_id,
        &workspace,
        &config.work_item_flow_defaults,
    )
    .await
    {
        tracing::warn!(
            user_id = %user_id,
            workspace = %workspace,
            error = %e,
            "Failed to seed default work-item flows for new user (continuing)"
        );
    }
}

/// `GET /api/users/{id}` -- Get a single user (admin only).
pub async fn get_user(
    State(auth): State<AuthState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Err(status) = require_admin(&auth, &headers).await {
        return status.into_response();
    }

    let db = match auth.db.as_ref() {
        Some(db) => db,
        None => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    match takuto_core::db::users::get_user_by_id(db.adapter(), &id).await {
        Ok(Some(user)) => Json(user).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// `PATCH /api/users/{id}` -- Update a user (admin only).
pub async fn update_user(
    State(auth): State<AuthState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<UpdateUserRequest>,
) -> impl IntoResponse {
    let admin = match require_admin(&auth, &headers).await {
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

    let db = match auth.db.as_ref() {
        Some(db) => db,
        None => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let adapter = db.adapter();

    let new_role = body.role;
    let new_username = body.username.clone();
    // Capture previous role + update + session invalidate via the
    // adapter. update_user opens its own internal tx for the last-admin
    // guard; the role-change session purge runs afterward inside a short
    // tx (so a delete failure doesn't leave a user with the wrong role +
    // stale sessions).
    let result: takuto_core::error::Result<takuto_core::db::models::User> = async {
        let previous_role = takuto_core::db::users::get_user_by_id(adapter, &id)
            .await
            .ok()
            .flatten()
            .map(|u| u.role);
        let updated =
            takuto_core::db::users::update_user(adapter, &id, new_username.as_deref(), new_role)
                .await?;
        let role_changed =
            matches!((previous_role, new_role), (Some(prev), Some(new)) if prev != new);
        if role_changed {
            let mut tx = adapter.begin().await?;
            let _ = takuto_core::db::credentials::delete_user_sessions(&mut tx, &id).await;
            tx.commit().await?;
            tracing::info!(
                event = "role_change_session_invalidate",
                user_id = %id,
                "deleted all sessions after role change"
            );
        }
        Ok(updated)
    }
    .await;

    match result {
        Ok(user) => Json(user).into_response(),
        Err(e) => {
            let msg = e.to_string();
            let status = if msg.contains("not found") || msg.contains("Not found") {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::BAD_REQUEST
            };
            (status, Json(serde_json::json!({"error": msg}))).into_response()
        }
    }
}

/// `POST /api/users/{id}/suspend` -- Suspend a user (admin only).
pub async fn suspend_user(
    State(auth): State<AuthState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let admin = match require_admin(&auth, &headers).await {
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

    let db = match auth.db.as_ref() {
        Some(db) => db,
        None => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let adapter = db.adapter();

    let result: takuto_core::error::Result<()> = async {
        takuto_core::db::users::suspend_user(adapter, &id).await?;
        let mut tx = adapter.begin().await?;
        takuto_core::db::credentials::delete_user_sessions(&mut tx, &id).await?;
        tx.commit().await?;
        Ok(())
    }
    .await;

    match result {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            let msg = e.to_string();
            let status = if msg.contains("not found") || msg.contains("Not found") {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::BAD_REQUEST
            };
            (status, Json(serde_json::json!({"error": msg}))).into_response()
        }
    }
}

/// `POST /api/users/{id}/unsuspend` -- Unsuspend a user (admin only).
pub async fn unsuspend_user(
    State(auth): State<AuthState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Err(status) = require_admin(&auth, &headers).await {
        return status.into_response();
    }

    let db = match auth.db.as_ref() {
        Some(db) => db,
        None => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    match takuto_core::db::users::unsuspend_user(db.adapter(), &id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            let msg = e.to_string();
            let status = if msg.contains("not found") || msg.contains("Not found") {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::BAD_REQUEST
            };
            (status, Json(serde_json::json!({"error": msg}))).into_response()
        }
    }
}

/// `POST /api/users/{id}/unlock` — clear all login-attempt rows for a user
/// (admin only).
///
/// Deletes both `kind = 'password'` and `kind = 'recovery'` rows so the user's
/// next login starts from a clean slate. Returns **204 No Content**.
pub async fn unlock_user(
    State(auth): State<AuthState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Err(status) = require_admin(&auth, &headers).await {
        return status.into_response();
    }

    let db = match auth.db.as_ref() {
        Some(db) => db,
        None => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let adapter = db.adapter();

    let exists = takuto_core::db::users::get_user_by_id(adapter, &id)
        .await
        .ok()
        .flatten()
        .is_some();

    let result: takuto_core::error::Result<()> = if exists {
        takuto_core::db::login_attempts::clear_attempts(adapter, &id).await
    } else {
        Err(AuthError::UserNotFound { id: id.clone() }.into())
    };

    match result {
        Ok(()) => {
            tracing::info!(
                event = "admin_unlock_user",
                user_id = %id,
                "cleared lockout counters for user"
            );
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => {
            let msg = e.to_string();
            let status = if msg.contains("not found") || msg.contains("Not found") {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::BAD_REQUEST
            };
            (status, Json(serde_json::json!({"error": msg}))).into_response()
        }
    }
}

/// `DELETE /api/users/{id}` -- Delete a user and all associated data (admin only).
pub async fn delete_user(
    State(auth): State<AuthState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let admin = match require_admin(&auth, &headers).await {
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

    let db = match auth.db.as_ref() {
        Some(db) => db,
        None => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    match takuto_core::db::users::delete_user(db.adapter(), &id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => {
            let msg = e.to_string();
            let status = if msg.contains("not found") || msg.contains("Not found") {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::BAD_REQUEST
            };
            (status, Json(serde_json::json!({"error": msg}))).into_response()
        }
    }
}

/// `GET /api/users/export` -- Export all users (admin only).
pub async fn export_users(State(auth): State<AuthState>, headers: HeaderMap) -> impl IntoResponse {
    if let Err(status) = require_admin(&auth, &headers).await {
        return status.into_response();
    }

    let db = match auth.db.as_ref() {
        Some(db) => db,
        None => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    match takuto_core::db::users::list_users(db.adapter()).await {
        Ok(users) => {
            let exports: Vec<UserExport> = users
                .into_iter()
                .map(|u| UserExport {
                    username: u.username,
                    role: u.role,
                    suspended: u.suspended,
                    created_at: u.created_at,
                })
                .collect();
            Json(exports).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

/// `POST /api/users/import` -- Import users (admin only).
///
/// Creates users from the import list. Skips users whose username already exists.
/// Imported users have no password set (admin must set one separately).
pub async fn import_users(
    State(auth): State<AuthState>,
    headers: HeaderMap,
    Json(body): Json<Vec<UserExport>>,
) -> impl IntoResponse {
    if let Err(status) = require_admin(&auth, &headers).await {
        return status.into_response();
    }

    let db = match auth.db.as_ref() {
        Some(db) => db,
        None => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let adapter = db.adapter();

    // Validation pass (existence checks) uses the adapter directly; the
    // insert pass runs in one explicit DbTransaction so all-or-nothing
    // atomicity is preserved.
    let result: takuto_core::error::Result<ImportSummary> = async {
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
            match takuto_core::db::users::get_user_by_username(adapter, username).await {
                Ok(Some(_)) => {
                    skipped.push(SkippedUser {
                        username: export.username.clone(),
                        reason: format!("Username '{}' already exists", username),
                    });
                }
                Ok(None) => to_create.push(export),
                Err(e) => return Err(e),
            }
        }

        let mut tx = adapter.begin().await?;
        let mut created = Vec::new();
        for export in to_create {
            let id = uuid::Uuid::new_v4().to_string();
            let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
            tx.execute(
                "INSERT INTO users (id, username, role, suspended, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?)",
                vec![
                    takuto_core::db::DbValue::Text(id),
                    takuto_core::db::DbValue::Text(export.username.trim().to_string()),
                    takuto_core::db::DbValue::Text(export.role.as_str().to_string()),
                    takuto_core::db::DbValue::I64(export.suspended as i64),
                    takuto_core::db::DbValue::Text(now.clone()),
                    takuto_core::db::DbValue::Text(now),
                ],
            )
            .await?;
            created.push(export.username.clone());
        }
        tx.commit().await?;

        Ok(ImportSummary { created, skipped })
    }
    .await;

    match result {
        Ok(summary) => (StatusCode::CREATED, Json(serde_json::json!(summary))).into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e.to_string()})),
        )
            .into_response(),
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

    use takuto_core::actions::dry_run::DryRunActions;
    use takuto_core::config::{Config, TicketingSystem};
    use takuto_core::db::Database;
    use takuto_core::workflow::engine::WorkflowEngine;

    use crate::server::build_router;
    use crate::state::AppState;

    /// Create a fresh Database backed by a temp directory (in-memory is `#[cfg(test)]`-gated
    /// in `takuto-core` and unavailable from downstream crate tests).
    fn temp_db() -> Database {
        let dir = std::env::temp_dir().join(format!("takuto-test-{}", uuid::Uuid::new_v4()));
        Database::open(&dir, true).expect("failed to create temp test database")
    }

    /// Create a test `AppState` with an in-memory SQLite database.
    fn test_state_with_db(db: Database) -> AppState {
        let config = Arc::new(RwLock::new(Config::default()));
        let actions: Arc<dyn takuto_core::actions::traits::ExternalActions> =
            Arc::new(DryRunActions::new("origin".to_string(), None));
        let jira_available = Arc::new(AtomicBool::new(false));
        let engine = Arc::new(WorkflowEngine::new(
            config.clone(),
            actions,
            1,
            jira_available.clone(),
            TicketingSystem::None,
            std::env::temp_dir(),
        ));
        let git_auth_resolver = Some(Arc::new(
            takuto_core::github::auth_resolver::GitAuthResolver::new(db.clone(), None),
        ));
        use crate::state::{AuthState, ConfigState, EditorState, EngineState, RunCommandState};
        AppState::new(
            EngineState {
                engine,
                polling_paused: Arc::new(AtomicBool::new(false)),
                clone_in_progress: Arc::new(AtomicBool::new(false)),
                system_status: Arc::new(RwLock::new(
                    takuto_core::docker_hooks::SystemStatus::default(),
                )),
            },
            AuthState {
                db: Some(db),
                gh_client: Arc::new(takuto_core::auth::RealGhClient::new()),
                git_auth_resolver,
            },
            ConfigState {
                config,
                config_path: std::env::temp_dir().join("config.toml"),
                config_writer: None,
                ticketing_system: TicketingSystem::None,
                jira_available,
                preflight_error: None,
                work_item_flow_defaults: std::sync::Arc::new(Vec::new()),
            },
            EditorState {
                editor_scanners: Arc::new(RwLock::new(HashMap::new())),
                dynamic_forwards: Arc::new(RwLock::new(HashMap::new())),
                terminal_ports: Arc::new(RwLock::new(HashMap::new())),
                editor_bundles: Arc::new(RwLock::new(HashMap::new())),
                path_token_registry: crate::session_registry::PathTokenRegistry::new(),
            },
            RunCommandState {
                run_commands: Arc::new(RwLock::new(HashMap::new())),
                run_command_bundles: Arc::new(RwLock::new(HashMap::new())),
            },
        )
    }

    /// Register the first user (admin) and return the DB session cookie value.
    async fn register_admin(state: &AppState) -> String {
        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::post("/api/auth/register")
                    .header("Content-Type", "application/json")
                    .header("Origin", "http://localhost:8080")
                    .body(Body::from(
                        r#"{"username":"admin","password":"secret123!@#A"}"#,
                    ))
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
                    .header("Origin", "http://localhost:8080")
                    .body(Body::from(
                        r#"{"username":"admin","password":"secret123!@#A"}"#,
                    ))
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
        takuto_core::db::users::get_user_by_username(db.adapter(), "admin")
            .await
            .unwrap()
            .unwrap()
            .id
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
                Request::post(format!("/api/users/{admin_id}/suspend"))
                    .header("Origin", "http://localhost:8080")
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
                    .uri(format!("/api/users/{admin_id}"))
                    .header("Origin", "http://localhost:8080")
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
                    .uri(format!("/api/users/{admin_id}"))
                    .header("Origin", "http://localhost:8080")
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
                    .header("Origin", "http://localhost:8080")
                    .header("Cookie", &cookie)
                    .header("Content-Type", "application/json")
                    .body(Body::from(
                        r#"{"username":"bob","password":"pass123!@#ABCD"}"#,
                    ))
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
                    .header("Origin", "http://localhost:8080")
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
