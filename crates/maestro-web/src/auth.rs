// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Dashboard session cookie auth (SQLite-backed multi-user).
//!
//! Session format: `db-<session-uuid>` — validated against the `sessions` table.

use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Request, State};
use axum::http::header::COOKIE;
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use maestro_core::auth::AuthError;
use maestro_core::config::WebConfig;
use maestro_core::db::Database;
use maestro_core::error::MaestroError;

use crate::state::AuthState;

mod test_clock {
    //! Test-only clock override (always compiled so integration tests in
    //! sibling crates can use it). Production code calls [`super::now_unix`]
    //! which checks this override first, so integration tests can drive the
    //! sliding-extend / absolute-TTL logic deterministically (G/W/T 5.4 / 5.5 /
    //! 5.7) without `tokio::time::pause`.
    //!
    //! The override is a single global `AtomicI64`; tests must run
    //! sequentially (the `cargo test` defaults) or use `serial_test` if
    //! parallelised. The override is never read in release builds where
    //! tests are not configured.
    use std::sync::atomic::{AtomicI64, Ordering};

    /// `i64::MIN` is the "unset" sentinel because `0` is a legitimate
    /// timestamp (the Unix epoch).
    static OVERRIDE: AtomicI64 = AtomicI64::new(i64::MIN);

    pub(super) fn current() -> Option<i64> {
        let v = OVERRIDE.load(Ordering::Relaxed);
        if v == i64::MIN { None } else { Some(v) }
    }

    pub fn set(t: i64) {
        OVERRIDE.store(t, Ordering::Relaxed);
    }

    pub fn clear() {
        OVERRIDE.store(i64::MIN, Ordering::Relaxed);
    }
}

/// Set the clock used by [`now_unix`] to a specific Unix-seconds value.
///
/// Test seam — production code should never call this. Integration tests use
/// it to drive the sliding-extend / absolute-TTL gates without sleeping.
pub fn set_test_now_unix(t: i64) {
    test_clock::set(t);
}

/// Clear the test-clock override; subsequent [`now_unix`] calls go back to wall-clock.
pub fn clear_test_now_unix() {
    test_clock::clear();
}

pub const SESSION_COOKIE_NAME: &str = "maestro_session";
/// Idle TTL — a session is rejected once it has been inactive for this
/// long. Sliding-extended by the auth middleware on each authenticated
/// request (gated by [`SESSION_EXTEND_THRESHOLD_SECS`] so the write rate
/// stays bounded). Equivalent to ~24 hours.
pub const SESSION_IDLE_TTL_SECS: u64 = 60 * 60 * 24;
/// Absolute TTL — sessions older than this are rejected even when actively
/// used, forcing periodic re-authentication. ~30 days.
pub const SESSION_ABSOLUTE_TTL_SECS: u64 = 60 * 60 * 24 * 30;
/// Minimum interval between `last_seen_at` writes from the auth
/// middleware. Prevents every authenticated request from issuing an
/// `UPDATE` against the sessions row.
pub const SESSION_EXTEND_THRESHOLD_SECS: u64 = 5 * 60;

pub fn session_cookie_from_headers(headers: &HeaderMap) -> Option<&str> {
    let raw = headers.get(COOKIE)?.to_str().ok()?;
    for part in raw.split(';') {
        let part = part.trim();
        let Some((name, value)) = part.split_once('=') else {
            continue;
        };
        if name.trim() == SESSION_COOKIE_NAME {
            return Some(value.trim().trim_matches('"'));
        }
    }
    None
}

/// Resolve whether session cookies should carry the `Secure` attribute.
///
/// Resolution order:
/// 1. If `web.cookie_secure` is `Some(v)` → `v` (explicit override always wins).
/// 2. Else if any `web.cors_origins` entry starts with `"https://"` → `true`.
/// 3. Else if the request has `X-Forwarded-Proto: https` (case-insensitive) → `true`.
/// 4. Otherwise → `false`.
///
/// This is used by every `Cookie::build(...)` site (login, logout, change_password)
/// and by the HSTS header in the security-headers middleware so they stay aligned.
pub fn resolve_cookie_secure(web: &WebConfig, headers: &HeaderMap) -> bool {
    if let Some(v) = web.cookie_secure {
        return v;
    }
    if web.cors_origins.iter().any(|o| o.starts_with("https://")) {
        return true;
    }
    if let Some(proto) = headers
        .get("X-Forwarded-Proto")
        .and_then(|v| v.to_str().ok())
        && proto.eq_ignore_ascii_case("https")
    {
        return true;
    }
    false
}

// ---------------------------------------------------------------------------
// Database-backed session management
// ---------------------------------------------------------------------------

/// Cookie value prefix for database-backed sessions.
///
/// Uses `db-` (not `db:`) because the `cookie` crate percent-encodes `:` as `%3A`
/// in `Set-Cookie` headers, which breaks prefix matching when the browser sends the
/// cookie back.
const DB_SESSION_PREFIX: &str = "db-";

/// Return the current Unix timestamp in seconds. Centralised so tests that
/// need to drive time forward have a single seam (see [`set_now_unix_override`]).
pub fn now_unix() -> i64 {
    if let Some(t) = test_clock::current() {
        return t;
    }
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Create a database-backed session for a user. Returns the session cookie value
/// (prefixed with `db-` so the middleware can distinguish it from legacy HMAC tokens).
///
/// The new row carries:
/// - `expires_at` — RFC3339 string, `now + SESSION_IDLE_TTL_SECS` (sliding TTL).
/// - `last_seen_at` — Unix seconds at creation (the middleware bumps this on use).
/// - `created_at_unix` — Unix seconds at creation; the absolute-TTL check
///   compares against this, so it must not be mutated for the lifetime of the
///   session. Sessions older than [`SESSION_ABSOLUTE_TTL_SECS`] are rejected
///   and lazily deleted even when actively used.
///
/// `expires_at` stays as a TEXT/RFC3339 string to match the schema (TEXT
/// NOT NULL in `schema.rs`). The lexicographic ordering of RFC3339 strings
/// matches chronological order, so comparisons in SQL and in Rust both work.
pub async fn create_db_session(
    adapter: &maestro_core::db::DbAdapter,
    user_id: &str,
) -> std::result::Result<String, MaestroError> {
    use maestro_core::db::DbValue;

    let session_id = uuid::Uuid::new_v4().to_string();
    let now_secs = now_unix();
    let expires_at_str = chrono::DateTime::<chrono::Utc>::from_timestamp(
        now_secs.saturating_add(SESSION_IDLE_TTL_SECS as i64),
        0,
    )
    .unwrap_or_else(chrono::Utc::now)
    .format("%Y-%m-%dT%H:%M:%SZ")
    .to_string();
    let data = serde_json::to_vec(&serde_json::json!({
        "user_id": user_id,
    }))
    .map_err(|source| AuthError::SessionSerialize { source })?;

    adapter
        .execute(
            "INSERT INTO sessions (id, user_id, data, expires_at, last_seen_at, created_at_unix) \
             VALUES (?, ?, ?, ?, ?, ?)",
            vec![
                DbValue::Text(session_id.clone()),
                DbValue::Text(user_id.to_string()),
                DbValue::Bytes(data),
                DbValue::Text(expires_at_str),
                DbValue::I64(now_secs),
                DbValue::I64(now_secs),
            ],
        )
        .await?;

    Ok(format!("{DB_SESSION_PREFIX}{session_id}"))
}

/// Validate a database session cookie and (when appropriate) slide its TTL forward.
///
/// Semantics:
/// 1. If `now - created_at_unix > SESSION_ABSOLUTE_TTL_SECS` → **delete** the row
///    and return `None`. The session is past its absolute 30-day cap.
/// 2. Else if the stored `expires_at` is in the past → return `None`. The
///    session has aged out idly; the row is left in place so a future
///    cleanup pass can reclaim it (no urgency — `validate` will not return
///    it again).
/// 3. Else, if `now - last_seen_at > SESSION_EXTEND_THRESHOLD_SECS`, update
///    `last_seen_at = now` and `expires_at = now + SESSION_IDLE_TTL_SECS`.
///    Returns the resolved `user_id`.
pub async fn validate_db_session(
    adapter: &maestro_core::db::DbAdapter,
    cookie_value: &str,
) -> Option<String> {
    use maestro_core::db::DbValue;

    let session_id = cookie_value.strip_prefix(DB_SESSION_PREFIX)?;
    let now_secs = now_unix();
    let now_rfc3339 = chrono::DateTime::<chrono::Utc>::from_timestamp(now_secs, 0)
        .unwrap_or_else(chrono::Utc::now)
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();

    // Fetch the user_id along with the lifecycle columns so all the gates
    // run against a single, consistent snapshot.
    let row = adapter
        .query_optional(
            "SELECT user_id, created_at_unix, last_seen_at, expires_at \
             FROM sessions WHERE id = ?",
            vec![DbValue::Text(session_id.to_string())],
        )
        .await
        .ok()
        .flatten()?;
    let user_id = row.get_text(0).ok()?;
    let created_at_unix = row.get_i64(1).ok()?;
    let last_seen_at = row.get_i64(2).ok()?;
    let expires_at = row.get_text(3).ok()?;

    // Absolute TTL — reject and lazily delete sessions older than 30 days,
    // even if they were recently used (G/W/T 5.7).
    if created_at_unix > 0
        && now_secs.saturating_sub(created_at_unix) > SESSION_ABSOLUTE_TTL_SECS as i64
    {
        let _ = adapter
            .execute(
                "DELETE FROM sessions WHERE id = ?",
                vec![DbValue::Text(session_id.to_string())],
            )
            .await;
        return None;
    }

    // Idle TTL — `expires_at` is an RFC3339 string; lexicographic comparison
    // matches chronological order for that format.
    if expires_at <= now_rfc3339 {
        return None;
    }

    // Sliding extend (G/W/T 5.4 / 5.5) — only write when we've crossed the
    // threshold, so light-load polling doesn't hammer the `sessions` table.
    if now_secs.saturating_sub(last_seen_at) > SESSION_EXTEND_THRESHOLD_SECS as i64 {
        let new_expires_at = chrono::DateTime::<chrono::Utc>::from_timestamp(
            now_secs.saturating_add(SESSION_IDLE_TTL_SECS as i64),
            0,
        )
        .unwrap_or_else(chrono::Utc::now)
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
        let _ = adapter
            .execute(
                "UPDATE sessions SET last_seen_at = ?, expires_at = ? WHERE id = ?",
                vec![
                    DbValue::I64(now_secs),
                    DbValue::Text(new_expires_at),
                    DbValue::Text(session_id.to_string()),
                ],
            )
            .await;
    }

    Some(user_id)
}

/// Read-only variant of [`validate_db_session`] — used by handlers that need
/// to check session validity but must not perform the sliding-extend `UPDATE`
/// (e.g. when the session is being deleted as part of the request anyway).
pub async fn validate_db_session_no_extend(
    adapter: &maestro_core::db::DbAdapter,
    cookie_value: &str,
) -> Option<String> {
    use maestro_core::db::DbValue;

    let session_id = cookie_value.strip_prefix(DB_SESSION_PREFIX)?;
    let now_secs = now_unix();
    let now_rfc3339 = chrono::DateTime::<chrono::Utc>::from_timestamp(now_secs, 0)
        .unwrap_or_else(chrono::Utc::now)
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();

    let row = adapter
        .query_optional(
            "SELECT user_id, created_at_unix, expires_at FROM sessions WHERE id = ?",
            vec![DbValue::Text(session_id.to_string())],
        )
        .await
        .ok()
        .flatten()?;
    let user_id = row.get_text(0).ok()?;
    let created_at_unix = row.get_i64(1).ok()?;
    let expires_at = row.get_text(2).ok()?;

    if created_at_unix > 0
        && now_secs.saturating_sub(created_at_unix) > SESSION_ABSOLUTE_TTL_SECS as i64
    {
        return None;
    }
    if expires_at <= now_rfc3339 {
        return None;
    }
    Some(user_id)
}

/// Delete a specific database session. Returns `true` if a session was deleted.
pub async fn delete_db_session(adapter: &maestro_core::db::DbAdapter, cookie_value: &str) -> bool {
    use maestro_core::db::DbValue;

    let Some(session_id) = cookie_value.strip_prefix(DB_SESSION_PREFIX) else {
        return false;
    };
    adapter
        .execute(
            "DELETE FROM sessions WHERE id = ?",
            vec![DbValue::Text(session_id.to_string())],
        )
        .await
        .map(|n| n > 0)
        .unwrap_or(false)
}

/// Authenticate a user against the SQLite database.
///
/// Returns the authenticated [`User`](maestro_core::db::models::User) on success,
/// or `None` if credentials are invalid or the user is suspended.
pub async fn authenticate_db_user(
    db: &Database,
    username: &str,
    password: &str,
) -> Option<maestro_core::db::models::User> {
    let adapter = db.adapter();
    let user = match maestro_core::db::users::get_user_by_username(adapter, username).await {
        Ok(Some(u)) => u,
        _ => return None,
    };
    if user.suspended {
        return None;
    }
    match maestro_core::db::credentials::verify_user_password(adapter, &user.id, password).await {
        Ok(true) => Some(user),
        _ => None,
    }
}

/// Authenticated user identity inserted into request extensions by the auth middleware.
/// Handlers extract this via `request.extensions().get::<AuthenticatedUser>()`.
#[derive(Debug, Clone)]
pub struct AuthenticatedUser {
    pub user_id: String,
    pub role: maestro_core::db::models::UserRole,
}

/// Database-backed multi-user auth middleware.
///
/// On successful authentication the middleware inserts an [`AuthenticatedUser`]
/// into the request extensions so downstream handlers can identify the caller.
/// If no database is available or no valid session exists, all protected
/// requests are rejected with 401.
pub async fn dashboard_auth_middleware(
    State(auth_state): State<AuthState>,
    mut request: Request,
    next: Next,
) -> Response {
    let Some(ref db) = auth_state.db else {
        // No database — reject all protected requests.
        return StatusCode::UNAUTHORIZED.into_response();
    };

    if let Some(raw_cookie) = session_cookie_from_headers(request.headers())
        && raw_cookie.starts_with(DB_SESSION_PREFIX)
    {
        let adapter = db.adapter();
        let user_id = validate_db_session(adapter, raw_cookie).await;
        let auth_user = if let Some(uid) = user_id {
            maestro_core::db::users::get_user_by_id(adapter, &uid)
                .await
                .ok()
                .flatten()
                .map(|user| AuthenticatedUser {
                    user_id: user.id,
                    role: user.role,
                })
        } else {
            None
        };

        if let Some(auth) = auth_user {
            request.extensions_mut().insert(auth);
            return next.run(request).await;
        }
    }

    StatusCode::UNAUTHORIZED.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_cookie_from_headers_extracts_value() {
        let mut headers = HeaderMap::new();
        headers.insert(
            COOKIE,
            "other=x; maestro_session=abc123.def456; foo=bar"
                .parse()
                .unwrap(),
        );
        assert_eq!(session_cookie_from_headers(&headers), Some("abc123.def456"));
    }

    #[test]
    fn session_cookie_from_headers_returns_none_when_missing() {
        let headers = HeaderMap::new();
        assert!(session_cookie_from_headers(&headers).is_none());
    }

    // -- resolve_cookie_secure --

    #[test]
    fn resolve_cookie_secure_default_plain_http_is_false() {
        let web = WebConfig::default();
        let headers = HeaderMap::new();
        assert!(!resolve_cookie_secure(&web, &headers));
    }

    #[test]
    fn resolve_cookie_secure_explicit_true_wins() {
        let web = WebConfig {
            cookie_secure: Some(true),
            ..Default::default()
        };
        let headers = HeaderMap::new();
        assert!(resolve_cookie_secure(&web, &headers));
    }

    #[test]
    fn resolve_cookie_secure_explicit_false_overrides_https_origin_and_header() {
        let web = WebConfig {
            cookie_secure: Some(false),
            cors_origins: vec!["https://x.example.com".into()],
            ..Default::default()
        };
        let mut headers = HeaderMap::new();
        headers.insert("X-Forwarded-Proto", "https".parse().unwrap());
        assert!(!resolve_cookie_secure(&web, &headers));
    }

    #[test]
    fn resolve_cookie_secure_https_cors_origin_triggers_true() {
        let web = WebConfig {
            cors_origins: vec!["https://maestro.example.com".into()],
            ..Default::default()
        };
        let headers = HeaderMap::new();
        assert!(resolve_cookie_secure(&web, &headers));
    }

    #[test]
    fn resolve_cookie_secure_forwarded_proto_https_triggers_true() {
        let web = WebConfig::default();
        let mut headers = HeaderMap::new();
        headers.insert("X-Forwarded-Proto", "https".parse().unwrap());
        assert!(resolve_cookie_secure(&web, &headers));
    }

    #[test]
    fn resolve_cookie_secure_forwarded_proto_case_insensitive() {
        let web = WebConfig::default();
        let mut headers = HeaderMap::new();
        headers.insert("X-Forwarded-Proto", "HTTPS".parse().unwrap());
        assert!(resolve_cookie_secure(&web, &headers));
    }

    #[test]
    fn resolve_cookie_secure_forwarded_proto_http_stays_false() {
        let web = WebConfig::default();
        let mut headers = HeaderMap::new();
        headers.insert("X-Forwarded-Proto", "http".parse().unwrap());
        assert!(!resolve_cookie_secure(&web, &headers));
    }

    #[test]
    fn resolve_cookie_secure_only_http_cors_origin_stays_false() {
        let web = WebConfig {
            cors_origins: vec!["http://localhost:3000".into()],
            ..Default::default()
        };
        let headers = HeaderMap::new();
        assert!(!resolve_cookie_secure(&web, &headers));
    }
}
