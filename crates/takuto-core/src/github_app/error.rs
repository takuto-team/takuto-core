// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Typed errors for the GitHub App authentication subsystem.
//!
//! Sub-enum that captures every distinct failure mode produced inside
//! `crates/takuto-core/src/github_app.rs`. Lifted from
//! `TakutoError::GitHubApp(String)` per the 2026-05-24 typed-errors-github-app
//! spec — every variant cites the call site it replaces so the migration
//! commits can be traced back.
//!
//! Wired into the workspace error envelope via
//! `TakutoError::GitHubApp(#[from] GitHubAppError)` so existing `?`
//! propagation across `Result<T, TakutoError>` boundaries keeps working
//! unchanged.
//!
//! # Foreign `#[from]` policy — none
//!
//! `jsonwebtoken::errors::Error` is referenced from two variants
//! (`InvalidPrivateKey`, `JwtSigning`); only one `#[from]` per source type is
//! legal under `thiserror`, so both use `#[source]` + explicit `.map_err(...)`.
//! `std::io::Error` collides with the envelope-level
//! `TakutoError::Io(#[from] std::io::Error)` and every github_app IO failure
//! needs path context, so all three IO variants use `#[source]` + `.map_err`.
//! `chrono::format::ParseError` needs the `raw` input string preserved, so it
//! likewise uses `#[source]` + `.map_err`.

use std::path::PathBuf;

/// Failures originating inside the GitHub App authentication subsystem.
/// Public for matching, but callers should generally just `?`-propagate into
/// a `TakutoError`.
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

#[cfg(test)]
mod tests {
    //! Lock-in tests for the typed GitHub App error surface.
    //!
    //! These tests pin two contracts against future drift:
    //!   1. The `Display` rendering of every `GitHubAppError` variant — the
    //!      messages flow into log lines and (via `TakutoError`) HTTP error
    //!      bodies, so a silent reword would be observable to operators.
    //!   2. The `#[from] GitHubAppError` chain into `TakutoError::GitHubApp(..)`
    //!      — every `?`-propagation inside `crates/takuto-core/src/github_app`
    //!      relies on this exact path; if a refactor accidentally wraps via a
    //!      different variant (e.g. the deprecated `GitHubAppStr` shim) these
    //!      tests fail.
    use super::*;
    use crate::error::TakutoError;

    /// Produce a deterministic `jsonwebtoken::errors::Error` for tests. We use
    /// `EncodingKey::from_rsa_pem` on obviously-bad bytes so the Display is
    /// whatever the upstream crate emits for malformed PEM input — pinned
    /// exactly to lock against silent dependency-version drift.
    fn sample_jwt_error() -> jsonwebtoken::errors::Error {
        match jsonwebtoken::EncodingKey::from_rsa_pem(b"not a pem") {
            Ok(_) => panic!("expected from_rsa_pem to reject malformed PEM"),
            Err(e) => e,
        }
    }

    /// Produce a deterministic `chrono::format::ParseError` for tests.
    fn sample_chrono_parse_error() -> chrono::format::ParseError {
        "not-a-date"
            .parse::<chrono::DateTime<chrono::Utc>>()
            .unwrap_err()
    }

    /// Produce a deterministic `std::io::Error` for tests. Display is not
    /// interpolated by any variant's `#[error(..)]` template, so the kind
    /// chosen here is irrelevant to the lock-in assertions.
    fn sample_io_error() -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::NotFound, "lock-in")
    }

    #[test]
    fn lock_in_github_app_error_display() {
        // 1. InvalidPrivateKey — interpolates {source}; pinned via real
        //    jsonwebtoken parse failure.
        let invalid_key = GitHubAppError::InvalidPrivateKey {
            source: sample_jwt_error(),
        };
        assert_eq!(
            format!("{}", invalid_key),
            format!(
                "invalid RSA private key in [github] config: {}",
                sample_jwt_error()
            )
        );

        // 2. PrivateKeyConfigConflict — static.
        assert_eq!(
            format!("{}", GitHubAppError::PrivateKeyConfigConflict),
            "set either [github] app_private_key or app_private_key_path, not both"
        );

        // 3. PrivateKeyRead — interpolates path.
        let private_key_read = GitHubAppError::PrivateKeyRead {
            path: PathBuf::from("/etc/takuto/gh-app.pem"),
            source: sample_io_error(),
        };
        assert_eq!(
            format!("{}", private_key_read),
            "cannot read [github] app_private_key_path /etc/takuto/gh-app.pem"
        );

        // 4. PrivateKeyMissing — static.
        assert_eq!(
            format!("{}", GitHubAppError::PrivateKeyMissing),
            "GitHub App private key not configured — set [github] app_private_key or app_private_key_path"
        );

        // 5. JwtSigning — static (no {source} interpolation).
        let jwt_signing = GitHubAppError::JwtSigning {
            source: sample_jwt_error(),
        };
        assert_eq!(
            format!("{}", jwt_signing),
            "failed to generate GitHub App JWT"
        );

        // 6. HttpRequestFailed — interpolates exit_code + stderr.
        let http_failed = GitHubAppError::HttpRequestFailed {
            exit_code: 22,
            stderr: "Could not resolve host: api.github.com".to_string(),
        };
        assert_eq!(
            format!("{}", http_failed),
            "curl request to GitHub API failed (exit 22): Could not resolve host: api.github.com"
        );

        // 7. ExpiresAtParse — interpolates raw (not source).
        let expires_at_parse = GitHubAppError::ExpiresAtParse {
            raw: "not-a-date".to_string(),
            source: sample_chrono_parse_error(),
        };
        assert_eq!(
            format!("{}", expires_at_parse),
            "failed to parse token expiry not-a-date"
        );

        // 8. ApiInstallationNotFound — interpolates installation_id (not doc URL).
        let api_not_found = GitHubAppError::ApiInstallationNotFound {
            installation_id: 999_888,
            documentation_url: "https://docs.github.com/rest".to_string(),
        };
        assert_eq!(
            format!("{}", api_not_found),
            "GitHub App installation not found (installation_id = 999888) — verify [github] app_installation_id is correct and the App is installed on your org/repo"
        );

        // 9. ApiJwtRejected — interpolates app_id + message.
        let api_jwt = GitHubAppError::ApiJwtRejected {
            app_id: 42,
            message: "could not be decoded".to_string(),
            documentation_url: "https://docs.github.com/rest".to_string(),
        };
        assert_eq!(
            format!("{}", api_jwt),
            "GitHub App JWT authentication failed (app_id = 42): could not be decoded"
        );

        // 10. ApiPermissionDenied — interpolates message.
        let api_perm = GitHubAppError::ApiPermissionDenied {
            message: "Resource not accessible by integration".to_string(),
            documentation_url: String::new(),
        };
        assert_eq!(
            format!("{}", api_perm),
            "GitHub App lacks required permissions: Resource not accessible by integration — needs contents (write), pull_requests (write), metadata (read)"
        );

        // 11. ApiOther — interpolates message.
        let api_other = GitHubAppError::ApiOther {
            message: "Something unexpected".to_string(),
            documentation_url: String::new(),
        };
        assert_eq!(
            format!("{}", api_other),
            "GitHub API error: Something unexpected"
        );

        // 12. UnexpectedApiResponse — interpolates body.
        let unexpected = GitHubAppError::UnexpectedApiResponse {
            body: "<html>404</html>".to_string(),
        };
        assert_eq!(
            format!("{}", unexpected),
            "unexpected GitHub API response: <html>404</html>"
        );

        // 13. GitConfigFailed — interpolates setting + stderr.
        let git_config = GitHubAppError::GitConfigFailed {
            setting: "user.name",
            stderr: "fatal: not in a git repo".to_string(),
        };
        assert_eq!(
            format!("{}", git_config),
            "git config user.name failed: fatal: not in a git repo"
        );

        // 14. TokenFileWrite — interpolates path (not source).
        let token_write = GitHubAppError::TokenFileWrite {
            path: PathBuf::from("/var/lib/takuto/gh-app-token"),
            source: sample_io_error(),
        };
        assert_eq!(
            format!("{}", token_write),
            "failed to write token file /var/lib/takuto/gh-app-token"
        );

        // 15. TokenFileRename — interpolates from + to (not source).
        let token_rename = GitHubAppError::TokenFileRename {
            from: PathBuf::from("/var/lib/takuto/gh-app-token.tmp"),
            to: PathBuf::from("/var/lib/takuto/gh-app-token"),
            source: sample_io_error(),
        };
        assert_eq!(
            format!("{}", token_rename),
            "failed to rename token file /var/lib/takuto/gh-app-token.tmp → /var/lib/takuto/gh-app-token"
        );
    }

    #[test]
    fn lock_in_github_app_error_into_takuto_error() {
        // Walk every variant through `TakutoError::from(..)` to guarantee the
        // `#[from] GitHubAppError` chain — not the deprecated `GitHubAppStr`
        // shim — is what `?`-propagation hits.

        let cases: Vec<GitHubAppError> = vec![
            GitHubAppError::InvalidPrivateKey {
                source: sample_jwt_error(),
            },
            GitHubAppError::PrivateKeyConfigConflict,
            GitHubAppError::PrivateKeyRead {
                path: PathBuf::from("/etc/takuto/gh-app.pem"),
                source: sample_io_error(),
            },
            GitHubAppError::PrivateKeyMissing,
            GitHubAppError::JwtSigning {
                source: sample_jwt_error(),
            },
            GitHubAppError::HttpRequestFailed {
                exit_code: 22,
                stderr: "Could not resolve host".to_string(),
            },
            GitHubAppError::ExpiresAtParse {
                raw: "not-a-date".to_string(),
                source: sample_chrono_parse_error(),
            },
            GitHubAppError::ApiInstallationNotFound {
                installation_id: 1,
                documentation_url: String::new(),
            },
            GitHubAppError::ApiJwtRejected {
                app_id: 1,
                message: "bad jwt".to_string(),
                documentation_url: String::new(),
            },
            GitHubAppError::ApiPermissionDenied {
                message: "perm".to_string(),
                documentation_url: String::new(),
            },
            GitHubAppError::ApiOther {
                message: "other".to_string(),
                documentation_url: String::new(),
            },
            GitHubAppError::UnexpectedApiResponse {
                body: "junk".to_string(),
            },
            GitHubAppError::GitConfigFailed {
                setting: "user.name",
                stderr: "stderr".to_string(),
            },
            GitHubAppError::TokenFileWrite {
                path: PathBuf::from("/tmp/t"),
                source: sample_io_error(),
            },
            GitHubAppError::TokenFileRename {
                from: PathBuf::from("/tmp/a"),
                to: PathBuf::from("/tmp/b"),
                source: sample_io_error(),
            },
        ];
        assert_eq!(cases.len(), 15, "must cover every GitHubAppError variant");

        for err in cases {
            let wrapped: TakutoError = err.into();
            assert!(
                matches!(
                    wrapped,
                    TakutoError::GitHubApp(
                        GitHubAppError::InvalidPrivateKey { .. }
                            | GitHubAppError::PrivateKeyConfigConflict
                            | GitHubAppError::PrivateKeyRead { .. }
                            | GitHubAppError::PrivateKeyMissing
                            | GitHubAppError::JwtSigning { .. }
                            | GitHubAppError::HttpRequestFailed { .. }
                            | GitHubAppError::ExpiresAtParse { .. }
                            | GitHubAppError::ApiInstallationNotFound { .. }
                            | GitHubAppError::ApiJwtRejected { .. }
                            | GitHubAppError::ApiPermissionDenied { .. }
                            | GitHubAppError::ApiOther { .. }
                            | GitHubAppError::UnexpectedApiResponse { .. }
                            | GitHubAppError::GitConfigFailed { .. }
                            | GitHubAppError::TokenFileWrite { .. }
                            | GitHubAppError::TokenFileRename { .. }
                    )
                ),
                "expected TakutoError::GitHubApp(GitHubAppError::<variant>), got {wrapped:?}"
            );
        }
    }
}
