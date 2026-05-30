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
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, Mac};
use maestro_core::auth::AuthError;
use maestro_core::config::WebConfig;
use maestro_core::db::Database;
use maestro_core::error::MaestroError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::state::AuthState;

type HmacSha256 = Hmac<Sha256>;

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
/// Plan-02 AC-5: idle TTL — a session is rejected once it has been inactive
/// for this long. Sliding-extended by the auth middleware on each authenticated
/// request (gated by [`SESSION_EXTEND_THRESHOLD_SECS`] so the write rate stays
/// bounded). Equivalent to ~24 hours.
pub const SESSION_IDLE_TTL_SECS: u64 = 60 * 60 * 24;
/// Plan-02 AC-5: absolute TTL — sessions older than this are rejected even
/// when actively used, forcing periodic re-authentication. ~30 days.
pub const SESSION_ABSOLUTE_TTL_SECS: u64 = 60 * 60 * 24 * 30;
/// Plan-02 AC-5: minimum interval between `last_seen_at` writes from the
/// auth middleware. Prevents every authenticated request from issuing an
/// `UPDATE` against the sessions row.
pub const SESSION_EXTEND_THRESHOLD_SECS: u64 = 5 * 60;

/// Legacy HMAC-signed session cookie TTL (the non-DB-backed code path). Kept
/// for the legacy `[web] dashboard_username` + `dashboard_password` mode that
/// pre-dates the multi-user database. New code uses the DB-backed session
/// flow with the constants above.
pub const SESSION_TTL_SECS: u64 = 60 * 60 * 24 * 7;

#[derive(Debug, Serialize, Deserialize)]
struct SessionClaims {
    exp: u64,
    sub: String,
}

fn hmac_key(password: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"maestro.dashboard.session.v1\0");
    h.update(password.as_bytes());
    h.finalize().into()
}

fn sign_message(key: &[u8; 32], msg: &[u8]) -> [u8; 32] {
    // SAFETY: `Hmac::new_from_slice` only fails on invalid key length, and
    // the type signature `&[u8; 32]` guarantees a 32-byte key — SHA-256
    // accepts any length up to its block size (64 bytes), so 32 is valid.
    let mut mac = HmacSha256::new_from_slice(key.as_slice())
        .expect("HMAC-SHA256 accepts any key length ≤ 64 bytes; key is &[u8; 32]");
    mac.update(msg);
    mac.finalize().into_bytes().into()
}

/// Constant-time verification of username and password against `[web]` dashboard fields.
pub fn credentials_match(web: &WebConfig, username: &str, password: &str) -> bool {
    let eu = web.dashboard_username.trim();
    let ep = web.dashboard_password.as_str();
    let u_ok = Sha256::digest(eu.as_bytes()).ct_eq(&Sha256::digest(username.trim().as_bytes()));
    let p_ok = Sha256::digest(ep.as_bytes()).ct_eq(&Sha256::digest(password.as_bytes()));
    (u_ok.into() && p_ok.into()) && web.dashboard_auth_enabled()
}

pub fn sign_session(username: &str, password: &str) -> Option<String> {
    let exp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_secs()
        .saturating_add(SESSION_TTL_SECS);
    let claims = serde_json::to_vec(&SessionClaims {
        exp,
        sub: username.to_string(),
    })
    .ok()?;
    let key = hmac_key(password);
    let sig = sign_message(&key, &claims);
    let b64 = URL_SAFE_NO_PAD.encode(&claims);
    Some(format!("{b64}.{}", hex::encode(sig)))
}

/// Validate cookie value: HMAC, expiry, and `sub` matches configured username (trimmed).
pub fn verify_session_cookie(raw: &str, web: &WebConfig) -> bool {
    if !web.dashboard_auth_enabled() {
        return false;
    }
    let (b64, sig_hex) = match raw.split_once('.') {
        Some((a, b)) if !a.is_empty() && !b.is_empty() => (a, b),
        _ => return false,
    };
    let Ok(sig_bytes) = hex::decode(sig_hex) else {
        return false;
    };
    if sig_bytes.len() != 32 {
        return false;
    }
    let Ok(claims_bytes): Result<Vec<u8>, _> = URL_SAFE_NO_PAD.decode(b64) else {
        return false;
    };
    let key = hmac_key(web.dashboard_password.as_str());
    let expected_sig = sign_message(&key, claims_bytes.as_slice());
    let sig_arr: [u8; 32] = match sig_bytes.as_slice().try_into() {
        Ok(a) => a,
        Err(_) => return false,
    };
    if !bool::from(expected_sig.ct_eq(&sig_arr)) {
        return false;
    }
    let Ok(claims) = serde_json::from_slice::<SessionClaims>(&claims_bytes) else {
        return false;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if claims.exp < now {
        return false;
    }
    let expected_user = web.dashboard_username.trim();
    Sha256::digest(expected_user.as_bytes())
        .ct_eq(&Sha256::digest(claims.sub.as_bytes()))
        .into()
}

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

pub fn session_authorized(headers: &HeaderMap, web: &WebConfig) -> bool {
    if !web.dashboard_auth_enabled() {
        return true;
    }
    session_cookie_from_headers(headers).is_some_and(|raw| verify_session_cookie(raw, web))
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
    if web
        .cors_origins
        .iter()
        .any(|o| o.starts_with("https://"))
    {
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
///   and lazily deleted even when actively used (plan-02 AC-5 / G/W/T 5.7).
pub fn create_db_session(
    conn: &rusqlite::Connection,
    user_id: &str,
) -> std::result::Result<String, MaestroError> {
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

    conn.execute(
        "INSERT INTO sessions (id, user_id, data, expires_at, last_seen_at, created_at_unix) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![session_id, user_id, data, expires_at_str, now_secs, now_secs],
    )?;

    Ok(format!("{DB_SESSION_PREFIX}{session_id}"))
}

/// Validate a database session cookie and (when appropriate) slide its TTL forward.
///
/// Plan-02 AC-5 semantics:
/// 1. If `now - created_at_unix > SESSION_ABSOLUTE_TTL_SECS` → **delete** the row
///    and return `None`. The session is past its absolute 30-day cap.
/// 2. Else if the stored `expires_at` is in the past → return `None`. The
///    session has aged out idly; the row is left in place so a future
///    cleanup pass can reclaim it (no urgency — `validate` will not return
///    it again).
/// 3. Else, if `now - last_seen_at > SESSION_EXTEND_THRESHOLD_SECS`, update
///    `last_seen_at = now` and `expires_at = now + SESSION_IDLE_TTL_SECS`.
///    Returns the resolved `user_id`.
pub fn validate_db_session(conn: &rusqlite::Connection, cookie_value: &str) -> Option<String> {
    let session_id = cookie_value.strip_prefix(DB_SESSION_PREFIX)?;
    let now_secs = now_unix();
    let now_rfc3339 = chrono::DateTime::<chrono::Utc>::from_timestamp(now_secs, 0)
        .unwrap_or_else(chrono::Utc::now)
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();

    // Fetch the user_id along with the lifecycle columns so all the gates
    // run against a single, consistent snapshot.
    let row: Option<(String, i64, i64, String)> = conn
        .query_row(
            "SELECT user_id, created_at_unix, last_seen_at, expires_at \
             FROM sessions WHERE id = ?1",
            rusqlite::params![session_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .ok();
    let (user_id, created_at_unix, last_seen_at, expires_at) = row?;

    // Absolute TTL — reject and lazily delete sessions older than 30 days,
    // even if they were recently used (G/W/T 5.7).
    if created_at_unix > 0
        && now_secs.saturating_sub(created_at_unix) > SESSION_ABSOLUTE_TTL_SECS as i64
    {
        let _ = conn.execute(
            "DELETE FROM sessions WHERE id = ?1",
            rusqlite::params![session_id],
        );
        return None;
    }

    // Idle TTL — the existing `expires_at` check rejects long-idle sessions.
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
        let _ = conn.execute(
            "UPDATE sessions SET last_seen_at = ?1, expires_at = ?2 WHERE id = ?3",
            rusqlite::params![now_secs, new_expires_at, session_id],
        );
    }

    Some(user_id)
}

/// Read-only variant of [`validate_db_session`] — used by handlers that need
/// to check session validity but must not perform the sliding-extend `UPDATE`
/// (e.g. when the session is being deleted as part of the request anyway).
pub fn validate_db_session_no_extend(
    conn: &rusqlite::Connection,
    cookie_value: &str,
) -> Option<String> {
    let session_id = cookie_value.strip_prefix(DB_SESSION_PREFIX)?;
    let now_secs = now_unix();
    let now_rfc3339 = chrono::DateTime::<chrono::Utc>::from_timestamp(now_secs, 0)
        .unwrap_or_else(chrono::Utc::now)
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();

    let row: Option<(String, i64, String)> = conn
        .query_row(
            "SELECT user_id, created_at_unix, expires_at FROM sessions WHERE id = ?1",
            rusqlite::params![session_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .ok();
    let (user_id, created_at_unix, expires_at) = row?;

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
pub fn delete_db_session(conn: &rusqlite::Connection, cookie_value: &str) -> bool {
    let Some(session_id) = cookie_value.strip_prefix(DB_SESSION_PREFIX) else {
        return false;
    };
    conn.execute(
        "DELETE FROM sessions WHERE id = ?1",
        rusqlite::params![session_id],
    )
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
    // Plan-11 step 3 cluster A: users + credentials on the agnostic
    // adapter. No spawn_blocking needed — direct async lookup + verify.
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
        // Plan-11 step 3 cluster A: users::get_user_by_id on the adapter.
        // Session lookup (validate_db_session — still rusqlite, sessions
        // table not yet migrated) stays in spawn_blocking.
        let db_clone = db.clone();
        let cookie = raw_cookie.to_string();
        let user_id = tokio::task::spawn_blocking(move || {
            let conn = db_clone.conn().blocking_lock();
            validate_db_session(&conn, &cookie)
        })
        .await
        .ok()
        .flatten();
        let auth_user = if let Some(uid) = user_id {
            maestro_core::db::users::get_user_by_id(db.adapter(), &uid)
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

    fn web_config_with_auth() -> WebConfig {
        WebConfig {
            dashboard_username: "admin".to_string(),
            dashboard_password: "secret123".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn sign_session_produces_non_empty_token() {
        let token = sign_session("admin", "secret123");
        assert!(token.is_some());
        let token = token.unwrap();
        assert!(!token.is_empty());
        assert!(token.contains('.'));
    }

    #[test]
    fn verify_session_cookie_accepts_valid_token() {
        let web = web_config_with_auth();
        let token = sign_session("admin", "secret123").unwrap();
        assert!(verify_session_cookie(&token, &web));
    }

    #[test]
    fn verify_session_cookie_rejects_unknown_token() {
        let web = web_config_with_auth();
        assert!(!verify_session_cookie("bogus.deadbeef", &web));
    }

    #[test]
    fn verify_session_cookie_rejects_wrong_password_signature() {
        let web = web_config_with_auth();
        let token = sign_session("admin", "wrong_password").unwrap();
        assert!(!verify_session_cookie(&token, &web));
    }

    #[test]
    fn verify_session_cookie_rejects_wrong_username() {
        let web = web_config_with_auth();
        // Sign with the right password but wrong username — HMAC matches (same password)
        // but the `sub` claim won't match the configured `dashboard_username`.
        let token = sign_session("hacker", "secret123").unwrap();
        assert!(!verify_session_cookie(&token, &web));
    }

    #[test]
    fn verify_session_cookie_rejects_empty_value() {
        let web = web_config_with_auth();
        assert!(!verify_session_cookie("", &web));
    }

    #[test]
    fn verify_session_cookie_false_when_auth_disabled() {
        let web = WebConfig::default(); // empty username + password
        assert!(!web.dashboard_auth_enabled());
        let token = sign_session("admin", "").unwrap();
        assert!(!verify_session_cookie(&token, &web));
    }

    #[test]
    fn credentials_match_returns_true_for_correct_creds() {
        let web = web_config_with_auth();
        assert!(credentials_match(&web, "admin", "secret123"));
    }

    #[test]
    fn credentials_match_returns_false_for_wrong_password() {
        let web = web_config_with_auth();
        assert!(!credentials_match(&web, "admin", "wrong"));
    }

    #[test]
    fn credentials_match_returns_false_for_wrong_username() {
        let web = web_config_with_auth();
        assert!(!credentials_match(&web, "hacker", "secret123"));
    }

    #[test]
    fn credentials_match_returns_false_when_auth_disabled() {
        let web = WebConfig::default();
        assert!(!credentials_match(&web, "", ""));
    }

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

    #[test]
    fn session_authorized_true_when_auth_disabled() {
        let web = WebConfig::default();
        let headers = HeaderMap::new();
        assert!(session_authorized(&headers, &web));
    }

    #[test]
    fn session_authorized_false_when_auth_enabled_no_cookie() {
        let web = web_config_with_auth();
        let headers = HeaderMap::new();
        assert!(!session_authorized(&headers, &web));
    }

    #[test]
    fn session_authorized_true_with_valid_cookie() {
        let web = web_config_with_auth();
        let token = sign_session("admin", "secret123").unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(COOKIE, format!("maestro_session={token}").parse().unwrap());
        assert!(session_authorized(&headers, &web));
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
