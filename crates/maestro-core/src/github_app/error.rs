// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Typed errors for the GitHub App authentication subsystem.
//!
//! Sub-enum that captures every distinct failure mode produced inside
//! `crates/maestro-core/src/github_app.rs`. Lifted from
//! `MaestroError::GitHubApp(String)` per the 2026-05-24 typed-errors-github-app
//! spec — every variant cites the call site it replaces so the migration
//! commits can be traced back.
//!
//! Wired into the workspace error envelope via
//! `MaestroError::GitHubApp(#[from] GitHubAppError)` so existing `?`
//! propagation across `Result<T, MaestroError>` boundaries keeps working
//! unchanged.
//!
//! # Foreign `#[from]` policy — none
//!
//! `jsonwebtoken::errors::Error` is referenced from two variants
//! (`InvalidPrivateKey`, `JwtSigning`); only one `#[from]` per source type is
//! legal under `thiserror`, so both use `#[source]` + explicit `.map_err(...)`.
//! `std::io::Error` collides with the envelope-level
//! `MaestroError::Io(#[from] std::io::Error)` and every github_app IO failure
//! needs path context, so all three IO variants use `#[source]` + `.map_err`.
//! `chrono::format::ParseError` needs the `raw` input string preserved, so it
//! likewise uses `#[source]` + `.map_err`.

use std::path::PathBuf;

/// Failures originating inside the GitHub App authentication subsystem.
/// Public for matching, but callers should generally just `?`-propagate into
/// a `MaestroError`.
#[derive(Debug, thiserror::Error)]
pub enum GitHubAppError {
    /// `github_app.rs:119` — `EncodingKey::from_rsa_pem` rejected the
    /// configured PEM bytes.
    #[error("invalid RSA private key in [github] config: {source}")]
    InvalidPrivateKey {
        #[source]
        source: jsonwebtoken::errors::Error,
    },

    /// `github_app.rs:161` — both `[github] app_private_key` and
    /// `app_private_key_path` are set; the config schema only permits one.
    #[error("set either [github] app_private_key or app_private_key_path, not both")]
    PrivateKeyConfigConflict,

    /// `github_app.rs:172` — `std::fs::read_to_string` failed for the
    /// configured `app_private_key_path`.
    #[error("cannot read [github] app_private_key_path {path}")]
    PrivateKeyRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// `github_app.rs:180` — neither `app_private_key` nor
    /// `app_private_key_path` is configured.
    #[error(
        "GitHub App private key not configured — set [github] app_private_key or app_private_key_path"
    )]
    PrivateKeyMissing,

    /// `github_app.rs:200` — `jsonwebtoken::encode` failed when signing the
    /// short-lived App JWT.
    #[error("failed to generate GitHub App JWT")]
    JwtSigning {
        #[source]
        source: jsonwebtoken::errors::Error,
    },

    /// `github_app.rs:272` — the `curl` invocation against the GitHub API
    /// exited non-zero.
    #[error("curl request to GitHub API failed (exit {exit_code}): {stderr}")]
    HttpRequestFailed { exit_code: i32, stderr: String },

    /// `github_app.rs:283` — the `expires_at` field in the installation-token
    /// response did not parse as RFC 3339.
    #[error("failed to parse token expiry {raw}")]
    ExpiresAtParse {
        raw: String,
        #[source]
        source: chrono::format::ParseError,
    },

    /// `github_app.rs:302` (API-error fanout, "Not Found" branch) — GitHub
    /// returned a 404 for the configured `installation_id`.
    #[error(
        "GitHub App installation not found (installation_id = {installation_id}) — verify [github] app_installation_id is correct and the App is installed on your org/repo"
    )]
    ApiInstallationNotFound {
        installation_id: u64,
        documentation_url: String,
    },

    /// `github_app.rs:302` (API-error fanout, "Unauthorized" branch) — GitHub
    /// rejected the App JWT.
    #[error("GitHub App JWT authentication failed (app_id = {app_id}): {message}")]
    ApiJwtRejected {
        app_id: u64,
        message: String,
        documentation_url: String,
    },

    /// `github_app.rs:302` (API-error fanout, "permission" branch) — the App
    /// installation is missing one of the required permission scopes.
    #[error(
        "GitHub App lacks required permissions: {message} — needs contents (write), pull_requests (write), metadata (read)"
    )]
    ApiPermissionDenied {
        message: String,
        documentation_url: String,
    },

    /// `github_app.rs:302` (API-error fanout, default branch) — GitHub
    /// returned a structured error that did not match any specialised arm.
    #[error("GitHub API error: {message}")]
    ApiOther {
        message: String,
        documentation_url: String,
    },

    /// `github_app.rs:305` — the GitHub API response body parsed as neither
    /// a successful token nor a structured API error.
    #[error("unexpected GitHub API response: {body}")]
    UnexpectedApiResponse { body: String },

    /// `github_app.rs:419` (`user.name`) and `github_app.rs:433`
    /// (`user.email`) — `git config <setting> …` exited non-zero.
    #[error("git config {setting} failed: {stderr}")]
    GitConfigFailed {
        setting: &'static str,
        stderr: String,
    },

    /// `github_app.rs:510` — `std::fs::write` failed for the temp sibling of
    /// the installation-token file.
    #[error("failed to write token file {path}")]
    TokenFileWrite {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// `github_app.rs:516` — `std::fs::rename` failed when promoting the
    /// temp sibling to the canonical installation-token path.
    #[error("failed to rename token file {from} → {to}")]
    TokenFileRename {
        from: PathBuf,
        to: PathBuf,
        #[source]
        source: std::io::Error,
    },
}
