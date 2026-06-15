// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Injectable shim around the `gh` CLI for per-user PAT validation.
//!
//! The PAT-save flow shells out to `gh api -i user` (and `gh api
//! orgs/<org>`) with `-H "Authorization: token <pat>"` so a freshly pasted
//! PAT can be validated without touching the host's `gh auth login`
//! credentials. The trait isolates the network from tests — every test
//! fixture wires a `MockGhClient` so the suite never hits github.com.
//!
//! Output parsing lives in [`crate::auth::pat_validation`]; this module owns
//! the I/O boundary only.

use std::process::{Command, Stdio};
use std::sync::Arc;

/// HTTP-ish response from one `gh api` invocation. Headers and body are kept
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
    /// Hit `gh api -i user` with the PAT as `Authorization: token <pat>`.
    /// Returns the parsed response (success + 401 + 403 all surface as
    /// `Ok(_)`); only an I/O / spawn failure produces `Err`.
    async fn api_user(&self, pat: &str) -> Result<GhResponse, String>;

    /// Hit `gh api orgs/<org>` with the PAT. Same conventions as
    /// [`api_user`] — 403 still returns `Ok(GhResponse{status:403,…})`.
    async fn api_org(&self, pat: &str, org: &str) -> Result<GhResponse, String>;
}

/// Production [`GhClient`] that shells out to the real `gh` binary on PATH.
///
/// Only the read endpoints (`api/user`, `api/orgs/<org>`) are used, over a
/// per-call PAT. The shim never persists the token; it lives only as a `-H`
/// arg for the duration of one `gh` invocation.
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

/// Parse `gh api -i <path>` output into a [`GhResponse`].
///
/// `gh -i` emits headers in HTTP-style (CRLF-separated) followed by a blank
/// line and the JSON body. The status line is `HTTP/2.0 <code> <reason>`.
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

/// Run `gh` synchronously with the given args (kept off the event loop by the
/// async wrapper). Returns combined stdout. `gh -i` writes the HTTP-style
/// response to **stdout**; stderr only carries CLI-level errors.
fn run_gh_blocking(args: &[&str]) -> Result<String, String> {
    let out = Command::new("gh")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("spawn gh failed: {e}"))?;
    // gh -i returns the HTTP body on stdout even for 4xx — we want that.
    // A non-zero exit with empty stdout means the CLI itself failed before
    // the API call (e.g. `gh` not installed). Surface as Err.
    if out.stdout.is_empty() && !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        return Err(format!("gh exited {}: {err}", out.status));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[async_trait::async_trait]
impl GhClient for RealGhClient {
    async fn api_user(&self, pat: &str) -> Result<GhResponse, String> {
        let pat = pat.to_string();
        let raw = tokio::task::spawn_blocking(move || {
            run_gh_blocking(&[
                "api",
                "-i",
                "--hostname",
                "github.com",
                "user",
                "-H",
                &format!("Authorization: token {pat}"),
            ])
        })
        .await
        .map_err(|e| format!("spawn_blocking join failed: {e}"))??;
        parse_gh_dash_i(&raw)
            .ok_or_else(|| format!("could not parse gh response: {} bytes", raw.len()))
    }

    async fn api_org(&self, pat: &str, org: &str) -> Result<GhResponse, String> {
        let pat = pat.to_string();
        let org = org.to_string();
        let raw = tokio::task::spawn_blocking(move || {
            run_gh_blocking(&[
                "api",
                "-i",
                "--hostname",
                "github.com",
                &format!("orgs/{org}"),
                "-H",
                &format!("Authorization: token {pat}"),
            ])
        })
        .await
        .map_err(|e| format!("spawn_blocking join failed: {e}"))??;
        parse_gh_dash_i(&raw)
            .ok_or_else(|| format!("could not parse gh response: {} bytes", raw.len()))
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
    fn parse_gh_dash_i_rejects_truncated_input() {
        assert!(parse_gh_dash_i("HTTP/2.0 200 OK\r\nbroken").is_none());
        assert!(parse_gh_dash_i("not http").is_none());
    }
}
