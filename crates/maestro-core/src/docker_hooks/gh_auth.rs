// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Recovery for expired GitHub App installation tokens via `gh auth switch`.

use std::process::{Command, Stdio};

use super::process::{auth_cmd_ok, preflight_home};

/// Try to recover from a failed `gh auth status` by switching to a user whose oauth token starts
/// with `gho_` (personal access token — does not expire). This handles the case where a GitHub App
/// installation token (`ghs_`) was set as the active user and has since expired.
///
/// Parses `~/.config/gh/hosts.yml` for the `github.com` host, finds any user with a `gho_` token,
/// and runs `gh auth switch --user <name> --hostname github.com`. Returns `true` if we switched and
/// `gh auth status` now passes.
// TODO: This uses a fragile line-based YAML parser with hardcoded indent levels (4/8/12 spaces)
// matching the current `gh` CLI output format. Consider using a YAML library if gh changes format.
pub(super) fn gh_auth_recover_expired_token() -> bool {
    let hosts_path = preflight_home().join(".config/gh/hosts.yml");
    let content = match std::fs::read_to_string(&hosts_path) {
        Ok(c) => c,
        Err(_) => return false,
    };

    // Minimal line-based parse — avoids a YAML dependency.
    // Expected structure (4-space indented YAML written by the gh CLI):
    //   github.com:
    //       users:
    //           morphet81:
    //               oauth_token: gho_...
    //           sous-coder[bot]:
    //               oauth_token: ghs_...
    let mut in_github_com = false;
    let mut in_users = false;
    let mut current_user: Option<String> = None;
    let mut personal_token_users: Vec<String> = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let indent = line.len() - line.trim_start().len();

        if !in_github_com {
            if trimmed == "github.com:" {
                in_github_com = true;
            }
            continue;
        }

        // A zero-indent non-comment line means we left the github.com block.
        if indent == 0 && !trimmed.starts_with('#') {
            break;
        }

        if trimmed == "users:" {
            in_users = true;
            current_user = None;
            continue;
        }

        if in_users {
            // A line at indent=4 that isn't "users:" signals we left the users block.
            if indent <= 4 && trimmed != "users:" {
                in_users = false;
                current_user = None;
                continue;
            }
            // Username entries sit at indent=8 and end with ':'
            if indent == 8 && trimmed.ends_with(':') {
                current_user = Some(trimmed.trim_end_matches(':').to_string());
                continue;
            }
            // Token lines are at indent=12
            if indent >= 12
                && let Some(ref user) = current_user
                && let Some(token) = trimmed.strip_prefix("oauth_token:")
            {
                let tok = token.trim();
                if tok.starts_with("gho_") {
                    personal_token_users.push(user.clone());
                }
            }
        }
    }

    for user in personal_token_users {
        let switched = Command::new("gh")
            .args([
                "auth",
                "switch",
                "--user",
                &user,
                "--hostname",
                "github.com",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if switched && auth_cmd_ok("gh", &["auth", "status"]) {
            eprintln!(
                "[maestro preflight] Auto-switched active gh user to '{user}' \
                 (previous token was expired or invalid — common with GitHub App installation tokens)."
            );
            return true;
        }
    }

    false
}
