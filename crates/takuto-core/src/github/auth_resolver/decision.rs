// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Pure decision layer for the `auth_resolver` module.
//!
//! Defines the three enums the resolver pivots on ([`GitAction`],
//! [`TokenSource`], [`GithubAuthMode`]) and the
//! [`decide_token_source`] function — a side-effect-free routing rule that
//! maps `(action, mode)` to a [`TokenSource`].
//!
//! No I/O, no async, no DB or network calls.

use std::fmt;

use super::GitAuthResult;

// ---------------------------------------------------------------------------
// Action / token / source / mode enums
// ---------------------------------------------------------------------------

/// Categorises every git operation the engine performs so the resolver can
/// route to the right token source. The variants mirror the rows of the
/// decision matrix in the parent module's doc-comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitAction {
    /// Read-only fetch from the remote.
    Clone,
    /// Read-only fetch (subsequent updates of a repo we already cloned).
    Fetch,
    /// Writes commits to the remote. Attributed to the user (UserPat) whenever
    /// a PAT is present, otherwise the App identity (bot).
    Push,
    /// `gh pr create` — opens a pull request.
    PullRequestCreate,
    /// Posts a comment on an existing pull request.
    PullRequestComment,
    /// Submits a review on an existing pull request.
    PullRequestReview,
    /// Comments on a GitHub issue.
    IssueComment,
    /// Webhook handler ingests payloads in the Takuto container — always
    /// uses the App identity since there's no user context.
    WebhookEventIngest,
}

impl GitAction {
    /// Stable identifier used in `tracing` lines.
    pub fn as_str(self) -> &'static str {
        match self {
            GitAction::Clone => "clone",
            GitAction::Fetch => "fetch",
            GitAction::Push => "push",
            GitAction::PullRequestCreate => "pull_request_create",
            GitAction::PullRequestComment => "pull_request_comment",
            GitAction::PullRequestReview => "pull_request_review",
            GitAction::IssueComment => "issue_comment",
            GitAction::WebhookEventIngest => "webhook_event_ingest",
        }
    }
}

/// The two token sources the resolver may return. Tests pattern-match on
/// this enum to check the decision matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenSource {
    /// GitHub App installation token (cached + auto-refreshed by
    /// `GitHubAppTokenManager`).
    App,
    /// User-supplied personal access token (sealed at rest in
    /// `user_github_credentials`).
    UserPat,
}

impl TokenSource {
    pub fn as_str(self) -> &'static str {
        match self {
            TokenSource::App => "app",
            TokenSource::UserPat => "user_pat",
        }
    }
}

/// Per-user GitHub auth mode. Mirrors the wire-format `github_mode` field
/// on `/api/auth/status` (Phase 0) so the UI can render the right hint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GithubAuthMode {
    /// `[github]` App is configured AND the user has a stored PAT.
    AppPlusPat,
    /// `[github]` App is configured; the user has no stored PAT.
    AppOnly,
    /// No App configured; the user has a stored PAT.
    PatOnly,
    /// Neither — every write action fails fast with
    /// `GitAuthError::UnauthenticatedGit`.
    Missing,
}

impl fmt::Display for GithubAuthMode {
    /// Wire-format string the dashboard renders.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            // Matches the existing Phase 0 vocabulary in
            // `crates/takuto-core/src/docker_hooks.rs::GitHubStatus.mode`.
            GithubAuthMode::AppPlusPat | GithubAuthMode::AppOnly => "app",
            GithubAuthMode::PatOnly => "pat_required",
            GithubAuthMode::Missing => "missing",
        };
        f.write_str(s)
    }
}

/// Pure decision function — does NOT contact the network or DB. Returns
/// `Some(source)` when the matrix has a verdict, `None` when no auth source
/// is available for the (action, mode) pair. The caller turns `None` into
/// `GitAuthError::UnauthenticatedGit`.
pub fn decide_token_source(
    action: GitAction,
    mode: GithubAuthMode,
    // No longer consulted: attribution is decided by PAT presence alone
    // (App+PAT → the user, for both commits and PRs; App-only → the bot).
    // Kept in the signature so existing callers/tests need not change.
    _attribute_commits: bool,
) -> GitAuthResult<Option<TokenSource>> {
    let source = match mode {
        GithubAuthMode::Missing => None,
        GithubAuthMode::AppOnly => Some(TokenSource::App),
        GithubAuthMode::PatOnly => Some(TokenSource::UserPat),
        GithubAuthMode::AppPlusPat => Some(match action {
            // Read-only + webhook → App (lower rate-limit cost, no user
            // attribution needed).
            GitAction::Clone | GitAction::Fetch | GitAction::WebhookEventIngest => TokenSource::App,
            // Every write is attributed to the user when a PAT exists —
            // pushing (commits), opening PRs, comments, reviews. Attribution is
            // determined solely by PAT presence (PAT → the user, for both
            // commits AND PRs; App-only → the bot), so Push uses the PAT here
            // like the rest of the writes.
            GitAction::Push
            | GitAction::PullRequestCreate
            | GitAction::PullRequestComment
            | GitAction::PullRequestReview
            | GitAction::IssueComment => TokenSource::UserPat,
        }),
    };
    Ok(source)
}

// ---------------------------------------------------------------------------
// Tests — decision matrix (28 cells) + Display
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// ── Pure decision function: 28 cells (7 actions × 4 modes) ──────────
    ///
    /// Each cell asserts:
    ///   (action, mode) → expected_source
    ///
    /// The `attribute_commits` argument is retained for signature
    /// compatibility but no longer affects the verdict; Push is tested with
    /// both flag values to lock that in.

    #[test]
    fn matrix_clone_mode_app_plus_pat_picks_app() {
        let got = decide_token_source(GitAction::Clone, GithubAuthMode::AppPlusPat, true).unwrap();
        assert_eq!(got, Some(TokenSource::App));
    }
    #[test]
    fn matrix_clone_mode_app_only_picks_app() {
        let got = decide_token_source(GitAction::Clone, GithubAuthMode::AppOnly, true).unwrap();
        assert_eq!(got, Some(TokenSource::App));
    }
    #[test]
    fn matrix_clone_mode_pat_only_picks_user_pat() {
        let got = decide_token_source(GitAction::Clone, GithubAuthMode::PatOnly, true).unwrap();
        assert_eq!(got, Some(TokenSource::UserPat));
    }
    #[test]
    fn matrix_clone_mode_missing_is_unauthenticated() {
        let got = decide_token_source(GitAction::Clone, GithubAuthMode::Missing, true).unwrap();
        assert_eq!(got, None);
    }

    #[test]
    fn matrix_fetch_mode_app_plus_pat_picks_app() {
        let got = decide_token_source(GitAction::Fetch, GithubAuthMode::AppPlusPat, true).unwrap();
        assert_eq!(got, Some(TokenSource::App));
    }
    #[test]
    fn matrix_fetch_mode_app_only_picks_app() {
        let got = decide_token_source(GitAction::Fetch, GithubAuthMode::AppOnly, true).unwrap();
        assert_eq!(got, Some(TokenSource::App));
    }
    #[test]
    fn matrix_fetch_mode_pat_only_picks_user_pat() {
        let got = decide_token_source(GitAction::Fetch, GithubAuthMode::PatOnly, true).unwrap();
        assert_eq!(got, Some(TokenSource::UserPat));
    }
    #[test]
    fn matrix_fetch_mode_missing_is_unauthenticated() {
        let got = decide_token_source(GitAction::Fetch, GithubAuthMode::Missing, true).unwrap();
        assert_eq!(got, None);
    }

    #[test]
    fn matrix_push_mode_app_plus_pat_picks_user_pat_regardless_of_attribute_flag() {
        // App+PAT attributes commits to the user (PAT present), exactly like a
        // PR. The legacy attribute flag no longer changes this.
        assert_eq!(
            decide_token_source(GitAction::Push, GithubAuthMode::AppPlusPat, true).unwrap(),
            Some(TokenSource::UserPat)
        );
        assert_eq!(
            decide_token_source(GitAction::Push, GithubAuthMode::AppPlusPat, false).unwrap(),
            Some(TokenSource::UserPat)
        );
    }
    #[test]
    fn matrix_push_mode_app_only_always_picks_app() {
        assert_eq!(
            decide_token_source(GitAction::Push, GithubAuthMode::AppOnly, true).unwrap(),
            Some(TokenSource::App)
        );
        assert_eq!(
            decide_token_source(GitAction::Push, GithubAuthMode::AppOnly, false).unwrap(),
            Some(TokenSource::App)
        );
    }
    #[test]
    fn matrix_push_mode_pat_only_always_picks_user_pat() {
        assert_eq!(
            decide_token_source(GitAction::Push, GithubAuthMode::PatOnly, true).unwrap(),
            Some(TokenSource::UserPat)
        );
        assert_eq!(
            decide_token_source(GitAction::Push, GithubAuthMode::PatOnly, false).unwrap(),
            Some(TokenSource::UserPat)
        );
    }
    #[test]
    fn matrix_push_mode_missing_is_unauthenticated_for_both_attribute_flags() {
        assert_eq!(
            decide_token_source(GitAction::Push, GithubAuthMode::Missing, true).unwrap(),
            None
        );
        assert_eq!(
            decide_token_source(GitAction::Push, GithubAuthMode::Missing, false).unwrap(),
            None
        );
    }

    #[test]
    fn matrix_pr_create_mode_app_plus_pat_picks_user_pat() {
        let got = decide_token_source(
            GitAction::PullRequestCreate,
            GithubAuthMode::AppPlusPat,
            true,
        )
        .unwrap();
        assert_eq!(got, Some(TokenSource::UserPat));
    }
    #[test]
    fn matrix_pr_create_mode_app_only_picks_app() {
        let got = decide_token_source(GitAction::PullRequestCreate, GithubAuthMode::AppOnly, true)
            .unwrap();
        assert_eq!(got, Some(TokenSource::App));
    }
    #[test]
    fn matrix_pr_create_mode_pat_only_picks_user_pat() {
        let got = decide_token_source(GitAction::PullRequestCreate, GithubAuthMode::PatOnly, true)
            .unwrap();
        assert_eq!(got, Some(TokenSource::UserPat));
    }
    #[test]
    fn matrix_pr_create_mode_missing_is_unauthenticated() {
        let got = decide_token_source(GitAction::PullRequestCreate, GithubAuthMode::Missing, true)
            .unwrap();
        assert_eq!(got, None);
    }

    #[test]
    fn matrix_pr_comment_mode_app_plus_pat_picks_user_pat() {
        let got = decide_token_source(
            GitAction::PullRequestComment,
            GithubAuthMode::AppPlusPat,
            true,
        )
        .unwrap();
        assert_eq!(got, Some(TokenSource::UserPat));
    }
    #[test]
    fn matrix_pr_comment_mode_app_only_picks_app() {
        let got = decide_token_source(GitAction::PullRequestComment, GithubAuthMode::AppOnly, true)
            .unwrap();
        assert_eq!(got, Some(TokenSource::App));
    }
    #[test]
    fn matrix_pr_comment_mode_pat_only_picks_user_pat() {
        let got = decide_token_source(GitAction::PullRequestComment, GithubAuthMode::PatOnly, true)
            .unwrap();
        assert_eq!(got, Some(TokenSource::UserPat));
    }
    #[test]
    fn matrix_pr_comment_mode_missing_is_unauthenticated() {
        let got = decide_token_source(GitAction::PullRequestComment, GithubAuthMode::Missing, true)
            .unwrap();
        assert_eq!(got, None);
    }

    #[test]
    fn matrix_pr_review_mode_app_plus_pat_picks_user_pat() {
        let got = decide_token_source(
            GitAction::PullRequestReview,
            GithubAuthMode::AppPlusPat,
            true,
        )
        .unwrap();
        assert_eq!(got, Some(TokenSource::UserPat));
    }
    #[test]
    fn matrix_pr_review_mode_app_only_picks_app() {
        let got = decide_token_source(GitAction::PullRequestReview, GithubAuthMode::AppOnly, true)
            .unwrap();
        assert_eq!(got, Some(TokenSource::App));
    }
    #[test]
    fn matrix_pr_review_mode_pat_only_picks_user_pat() {
        let got = decide_token_source(GitAction::PullRequestReview, GithubAuthMode::PatOnly, true)
            .unwrap();
        assert_eq!(got, Some(TokenSource::UserPat));
    }
    #[test]
    fn matrix_pr_review_mode_missing_is_unauthenticated() {
        let got = decide_token_source(GitAction::PullRequestReview, GithubAuthMode::Missing, true)
            .unwrap();
        assert_eq!(got, None);
    }

    #[test]
    fn matrix_issue_comment_mode_app_plus_pat_picks_user_pat() {
        let got =
            decide_token_source(GitAction::IssueComment, GithubAuthMode::AppPlusPat, true).unwrap();
        assert_eq!(got, Some(TokenSource::UserPat));
    }
    #[test]
    fn matrix_issue_comment_mode_app_only_picks_app() {
        let got =
            decide_token_source(GitAction::IssueComment, GithubAuthMode::AppOnly, true).unwrap();
        assert_eq!(got, Some(TokenSource::App));
    }
    #[test]
    fn matrix_issue_comment_mode_pat_only_picks_user_pat() {
        let got =
            decide_token_source(GitAction::IssueComment, GithubAuthMode::PatOnly, true).unwrap();
        assert_eq!(got, Some(TokenSource::UserPat));
    }
    #[test]
    fn matrix_issue_comment_mode_missing_is_unauthenticated() {
        let got =
            decide_token_source(GitAction::IssueComment, GithubAuthMode::Missing, true).unwrap();
        assert_eq!(got, None);
    }

    #[test]
    fn matrix_webhook_mode_app_plus_pat_picks_app() {
        let got = decide_token_source(
            GitAction::WebhookEventIngest,
            GithubAuthMode::AppPlusPat,
            true,
        )
        .unwrap();
        assert_eq!(got, Some(TokenSource::App));
    }
    #[test]
    fn matrix_webhook_mode_app_only_picks_app() {
        let got = decide_token_source(GitAction::WebhookEventIngest, GithubAuthMode::AppOnly, true)
            .unwrap();
        assert_eq!(got, Some(TokenSource::App));
    }
    #[test]
    fn matrix_webhook_mode_pat_only_picks_user_pat() {
        let got = decide_token_source(GitAction::WebhookEventIngest, GithubAuthMode::PatOnly, true)
            .unwrap();
        assert_eq!(got, Some(TokenSource::UserPat));
    }
    #[test]
    fn matrix_webhook_mode_missing_is_unauthenticated() {
        let got = decide_token_source(GitAction::WebhookEventIngest, GithubAuthMode::Missing, true)
            .unwrap();
        assert_eq!(got, None);
    }

    #[test]
    fn github_auth_mode_display_matches_phase_0_wire_strings() {
        assert_eq!(GithubAuthMode::AppPlusPat.to_string(), "app");
        assert_eq!(GithubAuthMode::AppOnly.to_string(), "app");
        assert_eq!(GithubAuthMode::PatOnly.to_string(), "pat_required");
        assert_eq!(GithubAuthMode::Missing.to_string(), "missing");
    }

    /// Lock-in test for the full `decide_token_source` truth table.
    ///
    /// The single-cell tests above protect against accidental tweaks to one
    /// row; this test pins the **entire** matrix in one `assert_eq!` so a
    /// reviewer can see — and a future refactor cannot quietly reshape —
    /// every `(GitAction × GithubAuthMode × attribute_commits)` outcome at
    /// once. If this fails, look at the diff: the right-hand side is the
    /// canonical decision matrix.
    #[test]
    fn lock_in_decision_table_exhaustive() {
        // 8 actions × 4 modes × 2 attribute_commits flags = 64 cells.
        let actions = [
            GitAction::Clone,
            GitAction::Fetch,
            GitAction::Push,
            GitAction::PullRequestCreate,
            GitAction::PullRequestComment,
            GitAction::PullRequestReview,
            GitAction::IssueComment,
            GitAction::WebhookEventIngest,
        ];
        let modes = [
            GithubAuthMode::AppPlusPat,
            GithubAuthMode::AppOnly,
            GithubAuthMode::PatOnly,
            GithubAuthMode::Missing,
        ];
        let attrs = [true, false];

        let mut actual: Vec<((GitAction, GithubAuthMode, bool), Option<TokenSource>)> = Vec::new();
        for &action in &actions {
            for &mode in &modes {
                for &attr in &attrs {
                    let got = decide_token_source(action, mode, attr).unwrap();
                    actual.push(((action, mode, attr), got));
                }
            }
        }

        // Canonical expected matrix: each row is
        //   ((action, mode, attribute_commits), Option<TokenSource>).
        let expected: Vec<((GitAction, GithubAuthMode, bool), Option<TokenSource>)> = vec![
            // Clone — read-only → App where App exists, else PAT, else None.
            (
                (GitAction::Clone, GithubAuthMode::AppPlusPat, true),
                Some(TokenSource::App),
            ),
            (
                (GitAction::Clone, GithubAuthMode::AppPlusPat, false),
                Some(TokenSource::App),
            ),
            (
                (GitAction::Clone, GithubAuthMode::AppOnly, true),
                Some(TokenSource::App),
            ),
            (
                (GitAction::Clone, GithubAuthMode::AppOnly, false),
                Some(TokenSource::App),
            ),
            (
                (GitAction::Clone, GithubAuthMode::PatOnly, true),
                Some(TokenSource::UserPat),
            ),
            (
                (GitAction::Clone, GithubAuthMode::PatOnly, false),
                Some(TokenSource::UserPat),
            ),
            ((GitAction::Clone, GithubAuthMode::Missing, true), None),
            ((GitAction::Clone, GithubAuthMode::Missing, false), None),
            // Fetch — same shape as Clone.
            (
                (GitAction::Fetch, GithubAuthMode::AppPlusPat, true),
                Some(TokenSource::App),
            ),
            (
                (GitAction::Fetch, GithubAuthMode::AppPlusPat, false),
                Some(TokenSource::App),
            ),
            (
                (GitAction::Fetch, GithubAuthMode::AppOnly, true),
                Some(TokenSource::App),
            ),
            (
                (GitAction::Fetch, GithubAuthMode::AppOnly, false),
                Some(TokenSource::App),
            ),
            (
                (GitAction::Fetch, GithubAuthMode::PatOnly, true),
                Some(TokenSource::UserPat),
            ),
            (
                (GitAction::Fetch, GithubAuthMode::PatOnly, false),
                Some(TokenSource::UserPat),
            ),
            ((GitAction::Fetch, GithubAuthMode::Missing, true), None),
            ((GitAction::Fetch, GithubAuthMode::Missing, false), None),
            // Push — attributed write: App+PAT → user (PAT present), like a PR.
            (
                (GitAction::Push, GithubAuthMode::AppPlusPat, true),
                Some(TokenSource::UserPat),
            ),
            (
                (GitAction::Push, GithubAuthMode::AppPlusPat, false),
                Some(TokenSource::UserPat),
            ),
            (
                (GitAction::Push, GithubAuthMode::AppOnly, true),
                Some(TokenSource::App),
            ),
            (
                (GitAction::Push, GithubAuthMode::AppOnly, false),
                Some(TokenSource::App),
            ),
            (
                (GitAction::Push, GithubAuthMode::PatOnly, true),
                Some(TokenSource::UserPat),
            ),
            (
                (GitAction::Push, GithubAuthMode::PatOnly, false),
                Some(TokenSource::UserPat),
            ),
            ((GitAction::Push, GithubAuthMode::Missing, true), None),
            ((GitAction::Push, GithubAuthMode::Missing, false), None),
            // PullRequestCreate — attributed write.
            (
                (
                    GitAction::PullRequestCreate,
                    GithubAuthMode::AppPlusPat,
                    true,
                ),
                Some(TokenSource::UserPat),
            ),
            (
                (
                    GitAction::PullRequestCreate,
                    GithubAuthMode::AppPlusPat,
                    false,
                ),
                Some(TokenSource::UserPat),
            ),
            (
                (GitAction::PullRequestCreate, GithubAuthMode::AppOnly, true),
                Some(TokenSource::App),
            ),
            (
                (GitAction::PullRequestCreate, GithubAuthMode::AppOnly, false),
                Some(TokenSource::App),
            ),
            (
                (GitAction::PullRequestCreate, GithubAuthMode::PatOnly, true),
                Some(TokenSource::UserPat),
            ),
            (
                (GitAction::PullRequestCreate, GithubAuthMode::PatOnly, false),
                Some(TokenSource::UserPat),
            ),
            (
                (GitAction::PullRequestCreate, GithubAuthMode::Missing, true),
                None,
            ),
            (
                (GitAction::PullRequestCreate, GithubAuthMode::Missing, false),
                None,
            ),
            // PullRequestComment.
            (
                (
                    GitAction::PullRequestComment,
                    GithubAuthMode::AppPlusPat,
                    true,
                ),
                Some(TokenSource::UserPat),
            ),
            (
                (
                    GitAction::PullRequestComment,
                    GithubAuthMode::AppPlusPat,
                    false,
                ),
                Some(TokenSource::UserPat),
            ),
            (
                (GitAction::PullRequestComment, GithubAuthMode::AppOnly, true),
                Some(TokenSource::App),
            ),
            (
                (
                    GitAction::PullRequestComment,
                    GithubAuthMode::AppOnly,
                    false,
                ),
                Some(TokenSource::App),
            ),
            (
                (GitAction::PullRequestComment, GithubAuthMode::PatOnly, true),
                Some(TokenSource::UserPat),
            ),
            (
                (
                    GitAction::PullRequestComment,
                    GithubAuthMode::PatOnly,
                    false,
                ),
                Some(TokenSource::UserPat),
            ),
            (
                (GitAction::PullRequestComment, GithubAuthMode::Missing, true),
                None,
            ),
            (
                (
                    GitAction::PullRequestComment,
                    GithubAuthMode::Missing,
                    false,
                ),
                None,
            ),
            // PullRequestReview.
            (
                (
                    GitAction::PullRequestReview,
                    GithubAuthMode::AppPlusPat,
                    true,
                ),
                Some(TokenSource::UserPat),
            ),
            (
                (
                    GitAction::PullRequestReview,
                    GithubAuthMode::AppPlusPat,
                    false,
                ),
                Some(TokenSource::UserPat),
            ),
            (
                (GitAction::PullRequestReview, GithubAuthMode::AppOnly, true),
                Some(TokenSource::App),
            ),
            (
                (GitAction::PullRequestReview, GithubAuthMode::AppOnly, false),
                Some(TokenSource::App),
            ),
            (
                (GitAction::PullRequestReview, GithubAuthMode::PatOnly, true),
                Some(TokenSource::UserPat),
            ),
            (
                (GitAction::PullRequestReview, GithubAuthMode::PatOnly, false),
                Some(TokenSource::UserPat),
            ),
            (
                (GitAction::PullRequestReview, GithubAuthMode::Missing, true),
                None,
            ),
            (
                (GitAction::PullRequestReview, GithubAuthMode::Missing, false),
                None,
            ),
            // IssueComment.
            (
                (GitAction::IssueComment, GithubAuthMode::AppPlusPat, true),
                Some(TokenSource::UserPat),
            ),
            (
                (GitAction::IssueComment, GithubAuthMode::AppPlusPat, false),
                Some(TokenSource::UserPat),
            ),
            (
                (GitAction::IssueComment, GithubAuthMode::AppOnly, true),
                Some(TokenSource::App),
            ),
            (
                (GitAction::IssueComment, GithubAuthMode::AppOnly, false),
                Some(TokenSource::App),
            ),
            (
                (GitAction::IssueComment, GithubAuthMode::PatOnly, true),
                Some(TokenSource::UserPat),
            ),
            (
                (GitAction::IssueComment, GithubAuthMode::PatOnly, false),
                Some(TokenSource::UserPat),
            ),
            (
                (GitAction::IssueComment, GithubAuthMode::Missing, true),
                None,
            ),
            (
                (GitAction::IssueComment, GithubAuthMode::Missing, false),
                None,
            ),
            // WebhookEventIngest — always App where App exists.
            (
                (
                    GitAction::WebhookEventIngest,
                    GithubAuthMode::AppPlusPat,
                    true,
                ),
                Some(TokenSource::App),
            ),
            (
                (
                    GitAction::WebhookEventIngest,
                    GithubAuthMode::AppPlusPat,
                    false,
                ),
                Some(TokenSource::App),
            ),
            (
                (GitAction::WebhookEventIngest, GithubAuthMode::AppOnly, true),
                Some(TokenSource::App),
            ),
            (
                (
                    GitAction::WebhookEventIngest,
                    GithubAuthMode::AppOnly,
                    false,
                ),
                Some(TokenSource::App),
            ),
            (
                (GitAction::WebhookEventIngest, GithubAuthMode::PatOnly, true),
                Some(TokenSource::UserPat),
            ),
            (
                (
                    GitAction::WebhookEventIngest,
                    GithubAuthMode::PatOnly,
                    false,
                ),
                Some(TokenSource::UserPat),
            ),
            (
                (GitAction::WebhookEventIngest, GithubAuthMode::Missing, true),
                None,
            ),
            (
                (
                    GitAction::WebhookEventIngest,
                    GithubAuthMode::Missing,
                    false,
                ),
                None,
            ),
        ];

        assert_eq!(actual, expected);
        // Sanity: 8 actions × 4 modes × 2 flags.
        assert_eq!(actual.len(), 64);
    }
}
