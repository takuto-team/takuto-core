// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Allowlisted `gh` (GitHub CLI) argv patterns.
//!
//! Maestro only issues a fixed set of `gh` subcommands unless
//! `[github] gh_allowed_extra_prefixes` extends the list (advanced).
//!
//! # Risk categories
//!
//! ## Read — no side effects
//! - `gh api user` — fetch authenticated user identity
//! - `gh api repos/{owner}/{repo}/issues` — list issues
//! - `gh api repos/{owner}/{repo}/pulls/{n}` — read PR status
//! - `gh auth status` — check auth status
//!
//! ## Write — creates or modifies resources
//! - `gh api --method PATCH repos/{owner}/{repo}/issues/{n}` — update issue description
//! - `gh pr create` — create a pull request
//! - `gh pr edit` — edit PR metadata (add reviewers, labels)
//!
//! ## Auth infrastructure — configures credentials (no data mutation)
//! - `gh auth login` — authenticate with GitHub
//! - `gh auth setup-git` — configure git credential helper
//!
//! ## Admin / Destructive — NOT allowed by default
//! - `gh repo delete` — delete a repository
//! - `gh api --method DELETE ...` — delete resources via API
//! - `gh auth logout` — revoke credentials
//! - `gh ssh-key add/delete` — manage SSH keys
//! - `gh secret set/delete` — manage repository secrets
//! - `gh pr merge` — merge a pull request
//! - `gh pr close` — close a pull request

use std::path::Path;

use tokio_util::sync::CancellationToken;

use crate::error::{MaestroError, Result};
use crate::process::{self, CommandOutput};

/// Built-in allowed argv prefixes (after the `gh` binary name).
///
/// Each entry is a prefix: if the argv starts with all tokens in a prefix,
/// the invocation is allowed. For example, `["api"]` allows `gh api user`,
/// `gh api repos/…/issues`, etc.
pub const GH_BUILTIN_PREFIXES: &[&[&str]] = &[
    &["api"],
    &["pr", "create"],
    &["pr", "edit"],
    &["auth", "login"],
    &["auth", "setup-git"],
    &["auth", "status"],
];

fn argv_starts_with(argv: &[&str], prefix: &[&str]) -> bool {
    if argv.len() < prefix.len() {
        return false;
    }
    argv.iter().zip(prefix.iter()).all(|(a, p)| *a == *p)
}

/// Returns `true` if the extra prefixes list contains a wildcard `["*"]` entry.
pub fn has_wildcard(extra_prefixes: &[Vec<String>]) -> bool {
    extra_prefixes
        .iter()
        .any(|p| p.len() == 1 && p[0] == "*")
}

/// Returns `true` if `argv` is allowed by built-in prefixes, extra prefixes, or wildcard.
pub fn gh_argv_allowed(argv: &[&str], extra_prefixes: &[Vec<String>]) -> bool {
    // Wildcard `["*"]` allows everything.
    if has_wildcard(extra_prefixes) {
        return true;
    }

    for p in GH_BUILTIN_PREFIXES {
        if argv_starts_with(argv, p) {
            return true;
        }
    }
    for ext in extra_prefixes {
        if ext.is_empty() {
            continue;
        }
        let pref: Vec<&str> = ext.iter().map(String::as_str).collect();
        if argv_starts_with(argv, &pref) {
            return true;
        }
    }
    false
}

/// Parse `[github] gh_allowed_extra_prefixes` entries into token vectors (whitespace-separated).
///
/// This is the same logic as `jira::acli::parse_acli_extra_prefixes` — split each line
/// on whitespace and filter empties.
pub fn parse_gh_extra_prefixes(raw: &[String]) -> Vec<Vec<String>> {
    raw.iter()
        .map(|s| {
            s.split_whitespace()
                .map(String::from)
                .filter(|t| !t.is_empty())
                .collect::<Vec<_>>()
        })
        .filter(|v| !v.is_empty())
        .collect()
}

/// Validate argv then spawn `gh` with `process::run_command` (no shell).
pub async fn run_gh_checked(
    argv: &[&str],
    extra_prefixes: &[Vec<String>],
    cwd: &Path,
    cancel: CancellationToken,
) -> Result<CommandOutput> {
    if !gh_argv_allowed(argv, extra_prefixes) {
        return Err(MaestroError::Git(format!(
            "blocked disallowed gh invocation (not on built-in allowlist or [github] gh_allowed_extra_prefixes): gh {}",
            argv.join(" ")
        )));
    }
    process::run_command("gh", argv, cwd, cancel).await
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Built-in prefix acceptance ──────────────────────────────────

    #[test]
    fn builtin_prefixes_allow_realistic_argv() {
        let cases: &[&[&str]] = &[
            // gh api user (identity lookup)
            &["api", "user"],
            // gh api repos/{}/issues (fetch_open_issues)
            &[
                "api",
                "--method",
                "GET",
                "repos/owner/repo/issues",
                "--field",
                "state=open",
                "--field",
                "per_page=50",
            ],
            // gh api repos/{}/pulls/42 --jq .merged (pr_merge_poller)
            &["api", "repos/owner/repo/pulls/42", "--jq", ".merged"],
            // gh api --method PATCH (update_ticket_description)
            &[
                "api",
                "--method",
                "PATCH",
                "repos/owner/repo/issues/7",
                "--raw-field",
                "body=text",
            ],
            // gh pr edit --add-reviewer (gh_github.rs)
            &[
                "pr",
                "edit",
                "https://github.com/o/r/pull/1",
                "--add-reviewer",
                "user",
            ],
            // gh pr create (real.rs)
            &[
                "pr",
                "create",
                "--title",
                "feat: add thing",
                "--body",
                "description",
                "--base",
                "main",
                "--head",
                "feat/x",
            ],
            // gh auth login --with-token (github_app.rs)
            &["auth", "login", "--with-token"],
            // gh auth setup-git (github_app.rs)
            &["auth", "setup-git"],
            // gh auth status (preflight reference)
            &["auth", "status"],
        ];
        for argv in cases {
            assert!(
                gh_argv_allowed(argv, &[]),
                "expected allowed: gh {}",
                argv.join(" ")
            );
        }
    }

    // ── Rejection / deny tests ──────────────────────────────────────

    #[test]
    fn empty_argv_not_allowed() {
        assert!(!gh_argv_allowed(&[], &[]));
    }

    #[test]
    fn dangerous_commands_blocked() {
        let cases: &[&[&str]] = &[
            &["repo", "delete", "owner/repo"],
            &["repo", "clone", "owner/repo"],
            &["ssh-key", "add"],
            &["secret", "set", "MY_SECRET"],
            &["run", "list"],
            &["pr", "merge", "42"],
            &["pr", "close", "42"],
            &["issue", "create", "--title", "x"],
            &["auth", "logout"],
            &["auth", "token"],
        ];
        for argv in cases {
            assert!(
                !gh_argv_allowed(argv, &[]),
                "expected blocked: gh {}",
                argv.join(" ")
            );
        }
    }

    #[test]
    fn pr_without_subcommand_blocked() {
        assert!(!gh_argv_allowed(&["pr"], &[]));
    }

    #[test]
    fn auth_without_subcommand_blocked() {
        assert!(!gh_argv_allowed(&["auth"], &[]));
    }

    #[test]
    fn partial_prefix_mismatch_blocked() {
        // "au" is not "auth"
        assert!(!gh_argv_allowed(&["au", "login"], &[]));
    }

    #[test]
    fn case_sensitive_mismatch_blocked() {
        assert!(!gh_argv_allowed(&["API", "user"], &[]));
        assert!(!gh_argv_allowed(&["Auth", "login"], &[]));
        assert!(!gh_argv_allowed(&["PR", "create"], &[]));
    }

    #[test]
    fn bare_api_matches_api_prefix() {
        // `["api"]` alone matches the `["api"]` prefix (gh will print help, harmless)
        assert!(gh_argv_allowed(&["api"], &[]));
    }

    // ── Extra prefix extension tests ────────────────────────────────

    #[test]
    fn extra_prefix_allows_custom_command() {
        let extra = vec![vec!["repo".into(), "view".into()]];
        assert!(gh_argv_allowed(
            &["repo", "view", "owner/repo"],
            &extra
        ));
    }

    #[test]
    fn extra_prefix_requires_full_match_not_partial() {
        let extra = vec![vec!["pr".into(), "list".into()]];
        assert!(!gh_argv_allowed(&["pr", "lis"], &extra));
    }

    #[test]
    fn extra_prefix_does_not_widen_existing() {
        // Adding extra ["pr", "edit"] is redundant; ["pr", "merge"] stays blocked
        let extra = vec![vec!["pr".into(), "edit".into()]];
        assert!(!gh_argv_allowed(&["pr", "merge", "42"], &extra));
    }

    #[test]
    fn argv_shorter_than_extra_prefix_not_allowed() {
        let extra = vec![vec!["repo".into(), "view".into(), "owner".into()]];
        assert!(!gh_argv_allowed(&["repo", "view"], &extra));
    }

    #[test]
    fn empty_extra_prefix_entry_skipped() {
        let extra: Vec<Vec<String>> = vec![vec![]];
        // Empty entry doesn't allow everything
        assert!(!gh_argv_allowed(&["repo", "delete"], &extra));
    }

    // ── Wildcard `"*"` tests ────────────────────────────────────────

    #[test]
    fn wildcard_allows_any_gh_command() {
        let extra = vec![vec!["*".into()]];
        assert!(gh_argv_allowed(&["repo", "clone", "evil"], &extra));
        assert!(gh_argv_allowed(&["secret", "set", "BOOM"], &extra));
    }

    #[test]
    fn wildcard_allows_empty_argv() {
        let extra = vec![vec!["*".into()]];
        assert!(gh_argv_allowed(&[], &extra));
    }

    #[test]
    fn wildcard_only_matches_exact_star_token() {
        // "*foo" is NOT a wildcard
        let extra = vec![vec!["*foo".into()]];
        assert!(!gh_argv_allowed(&["repo", "clone"], &extra));
    }

    #[test]
    fn wildcard_in_non_first_position_is_not_global() {
        // ["api", "*"] is just a two-token prefix, not a global wildcard
        let extra = vec![vec!["api".into(), "*".into()]];
        // "api *" matches "api * ..." but not "pr merge"
        assert!(!gh_argv_allowed(&["pr", "merge", "42"], &extra));
        // It does match argv starting with "api" and "*"
        assert!(gh_argv_allowed(&["api", "*", "anything"], &extra));
    }

    #[test]
    fn wildcard_with_other_prefixes() {
        // Wildcard combined with other entries still allows everything
        let extra = vec![vec!["repo".into(), "view".into()], vec!["*".into()]];
        assert!(gh_argv_allowed(&["secret", "delete"], &extra));
    }

    #[test]
    fn has_wildcard_detects_star() {
        assert!(has_wildcard(&[vec!["*".into()]]));
        assert!(!has_wildcard(&[]));
        assert!(!has_wildcard(&[vec!["api".into()]]));
        assert!(!has_wildcard(&[vec!["*foo".into()]]));
    }

    // ── Parse tests ─────────────────────────────────────────────────

    #[test]
    fn parse_extra_prefixes_trims_and_skips_empty() {
        let raw = vec![
            "  repo  view  ".to_string(),
            "".to_string(),
            "   \t  ".to_string(),
            "pr list".to_string(),
        ];
        let parsed = parse_gh_extra_prefixes(&raw);
        assert_eq!(parsed.len(), 2);
        assert_eq!(
            parsed[0],
            vec!["repo", "view"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            parsed[1],
            vec!["pr", "list"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn parse_extra_prefixes_empty_input() {
        assert!(parse_gh_extra_prefixes(&[]).is_empty());
        assert!(parse_gh_extra_prefixes(&["".into(), "  ".into()]).is_empty());
    }

    #[test]
    fn parse_extra_prefixes_wildcard_passthrough() {
        let raw = vec!["*".to_string()];
        let parsed = parse_gh_extra_prefixes(&raw);
        assert_eq!(parsed, vec![vec!["*".to_string()]]);
    }

    // ── Async wrapper tests ─────────────────────────────────────────

    #[tokio::test]
    async fn run_gh_checked_rejects_disallowed_before_spawn() {
        let tmp = tempfile::tempdir().unwrap();
        let err = run_gh_checked(
            &["repo", "clone", "evil/repo"],
            &[],
            tmp.path(),
            CancellationToken::new(),
        )
        .await
        .expect_err("disallowed argv should not spawn gh");

        let msg = err.to_string();
        assert!(
            msg.contains("blocked") && msg.contains("disallowed"),
            "unexpected error: {msg}"
        );
    }

    #[tokio::test]
    async fn run_gh_checked_does_not_block_allowlisted_argv() {
        let tmp = tempfile::tempdir().unwrap();
        // Allowlisted argv reaches `run_command` (may succeed, fail auth, or fail if `gh` is absent).
        let res = run_gh_checked(
            &["auth", "status"],
            &[],
            tmp.path(),
            CancellationToken::new(),
        )
        .await;

        if let Err(e) = res {
            let s = e.to_string();
            assert!(
                !s.contains("blocked disallowed gh"),
                "allowlisted argv must not be rejected by allowlist: {s}"
            );
        }
    }
}
