// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Injectable shim for per-user PAT validation against the GitHub REST API.
//!
//! The PAT-save flow issues `curl -i https://api.github.com/user` (and
//! `.../orgs/<org>`) with `-H "Authorization: token <pat>"` so a freshly
//! pasted PAT can be validated directly, without touching any host `gh auth
//! login` state. The trait isolates the network from tests — every test
//! fixture wires a `MockGhClient` so the suite never hits github.com.
//!
//! Response parsing lives in [`parse_gh_dash_i`]; this module owns the I/O
//! boundary only.

use std::process::{Command, Stdio};
use std::sync::Arc;

/// HTTP response from one GitHub API request. Headers and body are kept
/// raw so the parser can pick scopes and SSO URLs out of header lines.
#[derive(Debug, Clone)]
pub struct GhResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl GhResponse {
    /// Lookup a header value by case-insensitive name; returns the first match.
    pub fn header(&self, name: &str) -> Option<&str> {
        let lower = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| k.to_ascii_lowercase() == lower)
            .map(|(_, v)| v.as_str())
    }
}

/// Async trait so production impls can off-load to `tokio::task::spawn_blocking`
/// without blocking the executor on `gh` IPC. Tests use a synchronous mock
/// returning canned `GhResponse`s.
#[async_trait::async_trait]
pub trait GhClient: Send + Sync + 'static {
    /// GET `https://api.github.com/user` with the PAT as `Authorization: token
    /// <pat>`. Returns the parsed response (success + 401 + 403 all surface as
    /// `Ok(_)`); only an I/O / spawn failure produces `Err`.
    async fn api_user(&self, pat: &str) -> Result<GhResponse, String>;

    /// GET `https://api.github.com/orgs/<org>` with the PAT. Same conventions
    /// as [`api_user`] — 403 still returns `Ok(GhResponse{status:403,…})`.
    async fn api_org(&self, pat: &str, org: &str) -> Result<GhResponse, String>;
}

/// Production [`GhClient`] that shells out to `curl` against the GitHub API.
///
/// Only the read endpoints (`/user`, `/orgs/<org>`) are used, over a per-call
/// PAT. The shim never persists the token; it lives only as a `-H` arg for the
/// duration of one `curl` invocation.
pub struct RealGhClient;

impl RealGhClient {
    pub fn new() -> Self {
        Self
    }
}

impl Default for RealGhClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse `curl -i` (or `gh api -i`) output into a [`GhResponse`].
///
/// Both emit the status line, headers (CRLF-separated), a blank line, then the
/// JSON body. The status line is `HTTP/<ver> <code> [reason]` — `curl` over
/// HTTP/2 prints `HTTP/2 <code>` with no reason phrase, which still parses.
fn parse_gh_dash_i(raw: &str) -> Option<GhResponse> {
    // Find the blank line separating headers from body. Accept LF or CRLF.
    let (head, body) = match raw.find("\r\n\r\n") {
        Some(idx) => (&raw[..idx], &raw[idx + 4..]),
        None => match raw.find("\n\n") {
            Some(idx) => (&raw[..idx], &raw[idx + 2..]),
            None => return None,
        },
    };

    let mut lines = head.lines();
    let status_line = lines.next()?.trim();
    // "HTTP/2.0 200 OK" → 200; tolerate any spacing.
    let mut parts = status_line.split_whitespace();
    let _proto = parts.next()?;
    let status: u16 = parts.next()?.parse().ok()?;

    let mut headers = Vec::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }

    Some(GhResponse {
        status,
        headers,
        body: body.to_string(),
    })
}

/// GitHub REST API base. PAT validation only hits the public API host.
const GITHUB_API_BASE: &str = "https://api.github.com";

/// Hard ceiling (seconds) on a single GitHub API request, so a slow or
/// unreachable network can never park PAT validation forever.
const CURL_MAX_TIME_SECS: &str = "15";

/// Issue one `curl -i` GET against the GitHub API with the PAT as a bearer
/// header (kept off the event loop by the async wrapper). Returns curl's raw
/// stdout — the HTTP status line, headers, and body — for [`parse_gh_dash_i`].
///
/// We shell `curl` rather than the `gh` CLI: `curl` is universally present and
/// needs no auth/config/HOME context, whereas `gh` requires its own login
/// state and a PATH entry the server process frequently lacks (launchd /
/// Homebrew launcher locally, the worker user in a container), which surfaced
/// as spurious `gh_transport_error`s even with valid tokens and open egress.
fn run_curl_github_blocking(pat: &str, url: &str) -> Result<String, String> {
    let auth = format!("Authorization: token {pat}");
    let out = Command::new("curl")
        .args([
            "-sS",
            "-i",
            "--max-time",
            CURL_MAX_TIME_SECS,
            "-H",
            &auth,
            // GitHub rejects requests without a User-Agent.
            "-H",
            "User-Agent: takuto",
            "-H",
            "Accept: application/vnd.github+json",
            url,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("spawn curl failed: {e}"))?;
    // `curl -i` writes the full HTTP response (status + headers + body) to
    // stdout even for 4xx — we want that. A non-zero exit with empty stdout
    // means curl failed before any response (DNS/TLS/timeout). Surface as Err.
    if out.stdout.is_empty() && !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        return Err(format!("curl exited {}: {err}", out.status));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[async_trait::async_trait]
impl GhClient for RealGhClient {
    async fn api_user(&self, pat: &str) -> Result<GhResponse, String> {
        let pat = pat.to_string();
        let url = format!("{GITHUB_API_BASE}/user");
        let raw = tokio::task::spawn_blocking(move || run_curl_github_blocking(&pat, &url))
            .await
            .map_err(|e| format!("spawn_blocking join failed: {e}"))??;
        parse_gh_dash_i(&raw)
            .ok_or_else(|| format!("could not parse curl response: {} bytes", raw.len()))
    }

    async fn api_org(&self, pat: &str, org: &str) -> Result<GhResponse, String> {
        let pat = pat.to_string();
        let url = format!("{GITHUB_API_BASE}/orgs/{org}");
        let raw = tokio::task::spawn_blocking(move || run_curl_github_blocking(&pat, &url))
            .await
            .map_err(|e| format!("spawn_blocking join failed: {e}"))??;
        parse_gh_dash_i(&raw)
            .ok_or_else(|| format!("could not parse curl response: {} bytes", raw.len()))
    }
}

/// Convenience boxing for callers that pass the client into `AppState`.
pub type SharedGhClient = Arc<dyn GhClient>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_gh_dash_i_extracts_status_headers_body() {
        let raw = "HTTP/2.0 200 OK\r\n\
                   Content-Type: application/json\r\n\
                   X-OAuth-Scopes: repo, read:org\r\n\
                   \r\n\
                   {\"login\":\"alice\"}";
        let resp = parse_gh_dash_i(raw).expect("parse");
        assert_eq!(resp.status, 200);
        assert_eq!(resp.header("X-OAuth-Scopes"), Some("repo, read:org"));
        assert_eq!(resp.header("content-type"), Some("application/json"));
        assert!(resp.body.contains("\"login\""));
    }

    #[test]
    fn parse_gh_dash_i_accepts_lf_only_separator() {
        let raw = "HTTP/2.0 403 Forbidden\n\
                   X-GitHub-SSO: required; url=https://github.com/orgs/foo/sso?return_to=x\n\
                   \n\
                   {\"message\":\"sso\"}";
        let resp = parse_gh_dash_i(raw).expect("parse");
        assert_eq!(resp.status, 403);
        assert_eq!(
            resp.header("X-GitHub-SSO"),
            Some("required; url=https://github.com/orgs/foo/sso?return_to=x")
        );
    }

    #[test]
    fn parse_gh_dash_i_accepts_curl_http2_status_and_lowercase_headers() {
        // `curl -i` over HTTP/2 prints `HTTP/2 <code> ` (no reason phrase,
        // trailing space) and lowercases header names. Both must parse, and the
        // case-insensitive scope lookup must still resolve.
        let raw = "HTTP/2 200 \r\n\
                   content-type: application/json; charset=utf-8\r\n\
                   x-oauth-scopes: repo, read:org\r\n\
                   \r\n\
                   {\"login\":\"alice\"}";
        let resp = parse_gh_dash_i(raw).expect("parse");
        assert_eq!(resp.status, 200);
        assert_eq!(resp.header("X-OAuth-Scopes"), Some("repo, read:org"));
        assert!(resp.body.contains("\"login\""));
    }

    #[test]
    fn parse_gh_dash_i_rejects_truncated_input() {
        assert!(parse_gh_dash_i("HTTP/2.0 200 OK\r\nbroken").is_none());
        assert!(parse_gh_dash_i("not http").is_none());
    }
}
