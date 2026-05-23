// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Phase 2b.2 — `GitAuthResolver`. Picks the right token (GitHub App
//! installation token vs. per-user PAT) for a given [`GitAction`], based on
//! the per-user mode matrix in `tmp/multi-agents/04_architecture.md §4.2`.
//!
//! Scope: resolver + git-author plumbing. Worker container injection and
//! the per-workflow `auth_pin` ship in Phase 2b.3.
//!
//! Decision matrix (verbatim from arch doc §4.2):
//!
//! | `GitAction`                       | Mode B (App + PAT) | Mode A (App only) | Mode C (PAT only) | Missing |
//! |-----------------------------------|--------------------|-------------------|-------------------|---------|
//! | `Clone` / `Fetch`                 | App                | App               | UserPat           | Err     |
//! | `Push` (`attribute_commits=true`) | UserPat            | App               | UserPat           | Err     |
//! | `Push` (`attribute_commits=false`)| App                | App               | UserPat           | Err     |
//! | `PullRequestCreate`               | UserPat            | App               | UserPat           | Err     |
//! | `PullRequestComment` / `Review`   | UserPat            | App               | UserPat           | Err     |
//! | `IssueComment`                    | UserPat            | App               | UserPat           | Err     |
//! | `WebhookEventIngest`              | App                | App               | UserPat           | Err     |
//!
//! The implementation is split across five sibling modules; this file only
//! wires them up and re-exports the public surface so every existing
//! `crate::github::auth_resolver::*` path keeps resolving:
//!
//! - [`decision`] — pure `(action, mode, attribute_commits)` → `TokenSource`.
//! - [`errors`] — `GitAuthError`, `SecretToken`, `GitToken`, payload helper.
//! - [`audit`] — first-use debounce + `credential_audit` row writer.
//! - [`validator`] — PAT / SSO revalidation via `GhClient`.
//! - [`resolver`] — `GitAuthResolver` struct + orchestration impl.

pub mod audit;
pub mod decision;
pub mod errors;
pub mod resolver;
pub mod validator;

pub use decision::{decide_token_source, GitAction, GithubAuthMode, TokenSource};
pub use errors::{auth_warning_payload, GitAuthError, GitAuthResult, GitToken, SecretToken};
pub use resolver::GitAuthResolver;
