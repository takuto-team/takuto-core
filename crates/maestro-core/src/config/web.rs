// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! HTTP / WebSocket configuration plus dashboard runtime patches.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    /// When **both** `dashboard_username` and `dashboard_password` are set, the dashboard API and WebSocket require a signed session cookie (see `POST /api/auth/login`). Password is never returned by `GET /api/config`.
    #[serde(default)]
    pub dashboard_username: String,
    #[serde(default)]
    pub dashboard_password: String,
    /// Allowed CORS origins (e.g. `["http://localhost:8080", "https://maestro.example.com"]`).
    /// When empty (default), auto-computed from `host` and `port`.
    /// Startup-only — not patchable via `PUT /api/config`.
    #[serde(default)]
    pub cors_origins: Vec<String>,
    /// When set, controls the `Secure` flag on session cookies.
    /// `None` (default) auto-detects: `true` if any `cors_origins` entry is `https://…`
    /// or the inbound request carries `X-Forwarded-Proto: https`.
    #[serde(default)]
    pub cookie_secure: Option<bool>,
    /// Plan-02 AC-5: whether a successful login deletes prior sessions for the
    /// same user. Defaults to `true` (security-first). Set to `false` if your
    /// users routinely log in from multiple clients concurrently and the UX
    /// cost of forcing re-login on every new login outweighs the security
    /// benefit of single-session enforcement.
    #[serde(default = "default_kick_other_sessions")]
    pub kick_other_sessions_on_login: bool,
}

impl WebConfig {
    /// `true` when username (trimmed) and password are both non-empty.
    pub fn dashboard_auth_enabled(&self) -> bool {
        !self.dashboard_username.trim().is_empty() && !self.dashboard_password.is_empty()
    }

    /// Normalize `cors_origins` in place: strip default ports (:80 for http, :443 for https).
    /// Invalid entries are kept unchanged so that `Config::validate()` can report them as errors.
    /// Call this before `Config::validate()` so validation sees the canonical form.
    pub fn normalize_cors_origins(&mut self) {
        self.cors_origins = self
            .cors_origins
            .iter()
            .map(|o| validate_cors_origin(o).unwrap_or_else(|_| o.clone()))
            .collect();
    }

    /// Return the effective CORS origins: the explicit list if non-empty,
    /// otherwise a sensible default derived from `host` and `port`.
    pub fn resolved_cors_origins(&self) -> Vec<String> {
        if !self.cors_origins.is_empty() {
            return self.cors_origins.clone();
        }
        // Auto-compute: when binding to a wildcard or loopback address, the dashboard
        // is reachable via multiple hostnames (localhost, 127.0.0.1, 0.0.0.0, etc.).
        // Include all common variants so the CORS check passes regardless of which
        // hostname the operator typed in the browser address bar.
        let host = self.host.trim();
        let is_wildcard = host == "0.0.0.0" || host == "[::]";
        let is_loopback = host == "127.0.0.1" || host == "::1";
        if is_wildcard {
            vec![
                format!("http://localhost:{}", self.port),
                format!("http://127.0.0.1:{}", self.port),
                format!("http://0.0.0.0:{}", self.port),
            ]
        } else if is_loopback {
            // IPv6 addresses in URLs must be bracketed (RFC 2732).
            let host_part = if host.contains(':') {
                format!("[{}]", host)
            } else {
                host.to_string()
            };
            vec![
                format!("http://localhost:{}", self.port),
                format!("http://{}:{}", host_part, self.port),
            ]
        } else {
            // Bracket IPv6 literal addresses in the origin URL.
            let host_part = if host.contains(':') {
                format!("[{}]", host)
            } else {
                host.to_string()
            };
            vec![format!("http://{}:{}", host_part, self.port)]
        }
    }
}

/// Validate a single CORS origin string.
/// Must start with `http://` or `https://`, must have no path component (no `/` after the authority).
/// Normalizes default ports: strips `:80` from `http://` and `:443` from `https://`.
pub fn validate_cors_origin(origin: &str) -> std::result::Result<String, String> {
    let trimmed = origin.trim();
    if trimmed.is_empty() {
        return Err("[web] cors_origins: entry must not be empty".into());
    }

    let (scheme, authority) = if let Some(rest) = trimmed.strip_prefix("https://") {
        ("https", rest)
    } else if let Some(rest) = trimmed.strip_prefix("http://") {
        ("http", rest)
    } else {
        return Err(format!(
            "[web] cors_origins: '{trimmed}' must start with http:// or https://"
        ));
    };

    if authority.is_empty() {
        return Err(format!(
            "[web] cors_origins: '{trimmed}' has no host after scheme"
        ));
    }

    // Origins must not contain a path — no `/` in the authority portion.
    if authority.contains('/') {
        return Err(format!(
            "[web] cors_origins: '{trimmed}' must not contain a path (no '/' after the host)"
        ));
    }

    // Normalize default ports: strip :80 for http, :443 for https.
    let normalized = match scheme {
        "http" if authority.ends_with(":80") => {
            format!("http://{}", authority.strip_suffix(":80").unwrap())
        }
        "https" if authority.ends_with(":443") => {
            format!("https://{}", authority.strip_suffix(":443").unwrap())
        }
        _ => format!("{scheme}://{authority}"),
    };

    Ok(normalized)
}

fn default_host() -> String {
    "0.0.0.0".to_string()
}
fn default_port() -> u16 {
    8080
}
fn default_kick_other_sessions() -> bool {
    true
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            dashboard_username: String::new(),
            dashboard_password: String::new(),
            cors_origins: Vec::new(),
            cookie_secure: None,
            kick_other_sessions_on_login: default_kick_other_sessions(),
        }
    }
}

/// Dashboard `PUT /api/config` body: only these top-level keys are accepted (`deny_unknown_fields`).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeDashboardConfigPatch {
    #[serde(default)]
    pub web: Option<WebLoginPatch>,
    #[serde(default)]
    pub general: Option<GeneralConcurrencyPatch>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebLoginPatch {
    #[serde(default)]
    pub dashboard_username: Option<String>,
    #[serde(default)]
    pub dashboard_password: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneralConcurrencyPatch {
    #[serde(default)]
    pub max_concurrent_workflows: Option<u32>,
    #[serde(default)]
    pub max_active_workflows: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- resolved_cors_origins auto-computation --

    #[test]
    fn resolved_cors_origins_wildcard_includes_all_variants() {
        let web = WebConfig {
            host: "0.0.0.0".into(),
            port: 3000,
            cors_origins: Vec::new(),
            ..Default::default()
        };
        assert_eq!(
            web.resolved_cors_origins(),
            vec![
                "http://localhost:3000",
                "http://127.0.0.1:3000",
                "http://0.0.0.0:3000",
            ]
        );
    }

    #[test]
    fn resolved_cors_origins_ipv6_any_includes_all_variants() {
        let web = WebConfig {
            host: "[::]".into(),
            port: 8080,
            cors_origins: Vec::new(),
            ..Default::default()
        };
        assert_eq!(
            web.resolved_cors_origins(),
            vec![
                "http://localhost:8080",
                "http://127.0.0.1:8080",
                "http://0.0.0.0:8080",
            ]
        );
    }

    #[test]
    fn resolved_cors_origins_127001_includes_localhost() {
        let web = WebConfig {
            host: "127.0.0.1".into(),
            port: 9090,
            cors_origins: Vec::new(),
            ..Default::default()
        };
        assert_eq!(
            web.resolved_cors_origins(),
            vec!["http://localhost:9090", "http://127.0.0.1:9090"]
        );
    }

    #[test]
    fn resolved_cors_origins_ipv6_loopback_includes_localhost() {
        let web = WebConfig {
            host: "::1".into(),
            port: 4000,
            cors_origins: Vec::new(),
            ..Default::default()
        };
        assert_eq!(
            web.resolved_cors_origins(),
            vec!["http://localhost:4000", "http://[::1]:4000"]
        );
    }

    #[test]
    fn resolved_cors_origins_specific_host() {
        let web = WebConfig {
            host: "10.0.0.5".into(),
            port: 8080,
            cors_origins: Vec::new(),
            ..Default::default()
        };
        assert_eq!(web.resolved_cors_origins(), vec!["http://10.0.0.5:8080"]);
    }

    #[test]
    fn resolved_cors_origins_returns_explicit_list() {
        let web = WebConfig {
            host: "0.0.0.0".into(),
            port: 8080,
            cors_origins: vec![
                "https://app.example.com".into(),
                "http://localhost:3000".into(),
            ],
            ..Default::default()
        };
        let resolved = web.resolved_cors_origins();
        assert_eq!(
            resolved,
            vec!["https://app.example.com", "http://localhost:3000"]
        );
    }

    // -- Normalization --

    #[test]
    fn normalize_cors_origins_strips_http_port_80() {
        let mut web = WebConfig {
            cors_origins: vec!["http://example.com:80".into()],
            ..Default::default()
        };
        web.normalize_cors_origins();
        assert_eq!(web.cors_origins, vec!["http://example.com"]);
    }

    #[test]
    fn normalize_cors_origins_strips_https_port_443() {
        let mut web = WebConfig {
            cors_origins: vec!["https://example.com:443".into()],
            ..Default::default()
        };
        web.normalize_cors_origins();
        assert_eq!(web.cors_origins, vec!["https://example.com"]);
    }

    #[test]
    fn normalize_cors_origins_preserves_non_default_port() {
        let mut web = WebConfig {
            cors_origins: vec!["http://example.com:8080".into()],
            ..Default::default()
        };
        web.normalize_cors_origins();
        assert_eq!(web.cors_origins, vec!["http://example.com:8080"]);
    }
}
