// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Dashboard session cookie auth (username + password via `POST /api/auth/login`).

use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Request, State};
use axum::http::header::COOKIE;
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, Mac};
use maestro_core::config::WebConfig;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::state::AppState;

type HmacSha256 = Hmac<Sha256>;

pub const SESSION_COOKIE_NAME: &str = "maestro_session";
/// Session lifetime for signed cookie.
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
    let mut mac = HmacSha256::new_from_slice(key.as_slice()).expect("HMAC key length");
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

pub async fn dashboard_auth_middleware(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let web = {
        let cfg = state.config.read().await;
        cfg.web.clone()
    };
    if !web.dashboard_auth_enabled() {
        return next.run(request).await;
    }
    if !session_authorized(request.headers(), &web) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    next.run(request).await
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
}
