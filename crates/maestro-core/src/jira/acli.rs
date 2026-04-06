//! Allowlisted `acli` argv patterns. Maestro only issues Jira read/assign/transition commands unless
//! `[jira] acli_allowed_extra_prefixes` extends the list (advanced).

use std::path::Path;

use tokio_util::sync::CancellationToken;

use crate::error::{MaestroError, Result};
use crate::process::{self, CommandOutput};

/// Built-in allowed argv prefixes (after the `acli` binary name).
const BUILTIN_PREFIXES: &[&[&str]] = &[
    &["jira", "workitem", "search"],
    &["jira", "workitem", "view"],
    &["jira", "workitem", "assign"],
    &["jira", "workitem", "transition"],
    &["jira", "auth", "status"],
];

fn argv_starts_with(argv: &[&str], prefix: &[&str]) -> bool {
    if argv.len() < prefix.len() {
        return false;
    }
    argv.iter().zip(prefix.iter()).all(|(a, p)| *a == *p)
}

/// Returns true if `argv` is allowed by built-in prefixes or any entry in `extra_prefixes`.
pub fn acli_argv_allowed(argv: &[&str], extra_prefixes: &[Vec<String>]) -> bool {
    for p in BUILTIN_PREFIXES {
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

/// Parse `[jira] acli_allowed_extra_prefixes` entries into token vectors (whitespace-separated).
pub fn parse_acli_extra_prefixes(raw: &[String]) -> Vec<Vec<String>> {
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

/// Validate argv then spawn `acli` with `process::run_command` (no shell).
pub async fn run_acli_checked(
    argv: &[&str],
    extra_prefixes: &[Vec<String>],
    repo_path: &Path,
    cancel: CancellationToken,
) -> Result<CommandOutput> {
    if !acli_argv_allowed(argv, extra_prefixes) {
        return Err(MaestroError::Jira(format!(
            "blocked disallowed acli invocation (not on built-in allowlist or [jira] acli_allowed_extra_prefixes): acli {}",
            argv.join(" ")
        )));
    }
    process::run_command("acli", argv, repo_path, cancel).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_prefixes_allow_realistic_argv() {
        let cases: &[&[&str]] = &[
            &[
                "jira",
                "workitem",
                "search",
                "--jql",
                "project = X",
                "--json",
                "--limit",
                "50",
            ],
            &[
                "jira",
                "workitem",
                "view",
                "PROJ-1",
                "--json",
                "--fields",
                "key,summary",
            ],
            &[
                "jira",
                "workitem",
                "assign",
                "--key",
                "PROJ-1",
                "--assignee",
                "@me",
                "--yes",
            ],
            &[
                "jira",
                "workitem",
                "assign",
                "--key",
                "PROJ-1",
                "--remove-assignee",
                "--yes",
            ],
            &[
                "jira",
                "workitem",
                "transition",
                "--key",
                "PROJ-1",
                "--status",
                "In Progress",
                "--yes",
            ],
            &["jira", "auth", "status"],
        ];
        for argv in cases {
            assert!(
                acli_argv_allowed(argv, &[]),
                "expected allowed: acli {}",
                argv.join(" ")
            );
        }
    }

    #[test]
    fn empty_argv_not_allowed() {
        assert!(!acli_argv_allowed(&[], &[]));
    }

    #[test]
    fn jira_workitem_without_subcommand_not_allowed() {
        assert!(!acli_argv_allowed(&["jira", "workitem"], &[]));
    }

    #[test]
    fn delete_and_edit_blocked() {
        for argv in [
            &["jira", "workitem", "delete", "--key", "X"][..],
            &["jira", "workitem", "edit", "--key", "X"][..],
            &["jira", "workitem", "move", "--key", "X"][..],
        ] {
            assert!(
                !acli_argv_allowed(argv, &[]),
                "expected blocked: acli {}",
                argv.join(" ")
            );
        }
    }

    #[test]
    fn non_jira_top_level_blocked() {
        assert!(!acli_argv_allowed(&["confluence", "content", "list"], &[]));
    }

    #[test]
    fn typo_in_workitem_blocked() {
        assert!(!acli_argv_allowed(&["jira", "workitems", "view", "X"], &[]));
    }

    #[test]
    fn extra_prefix_allows_subcommand() {
        let extra = vec![vec!["jira".into(), "workitem".into(), "comment".into()]];
        assert!(acli_argv_allowed(
            &["jira", "workitem", "comment", "--key", "X", "--body", "hi"],
            &extra
        ));
    }

    #[test]
    fn extra_prefix_requires_full_match_not_partial() {
        let extra = vec![vec!["jira".into(), "workitem".into(), "comment".into()]];
        assert!(!acli_argv_allowed(
            &["jira", "workitem", "comm", "x"],
            &extra
        ));
    }

    #[test]
    fn argv_shorter_than_extra_prefix_not_allowed() {
        let extra = vec![vec![
            "jira".into(),
            "workitem".into(),
            "comment".into(),
            "extra".into(),
        ]];
        assert!(!acli_argv_allowed(&["jira", "workitem", "comment"], &extra));
    }

    #[test]
    fn parse_extra_prefixes_trims_and_skips_empty_lines() {
        let raw = vec![
            "  jira  workitem  comment  ".to_string(),
            "".to_string(),
            "   \t  ".to_string(),
            "jira admin users".to_string(),
        ];
        let parsed = parse_acli_extra_prefixes(&raw);
        assert_eq!(parsed.len(), 2);
        assert_eq!(
            parsed[0],
            vec!["jira", "workitem", "comment"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            parsed[1],
            vec!["jira", "admin", "users"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn parse_extra_prefixes_empty_input() {
        assert!(parse_acli_extra_prefixes(&[]).is_empty());
        assert!(parse_acli_extra_prefixes(&["".into(), "  ".into()]).is_empty());
    }

    #[tokio::test]
    async fn run_acli_checked_rejects_disallowed_before_spawn() {
        let tmp = tempfile::tempdir().unwrap();
        let err = run_acli_checked(
            &["jira", "workitem", "delete", "--key", "X"],
            &[],
            tmp.path(),
            CancellationToken::new(),
        )
        .await
        .expect_err("disallowed argv should not spawn acli");

        let msg = err.to_string();
        assert!(
            msg.contains("blocked") && msg.contains("disallowed"),
            "unexpected error: {msg}"
        );
    }

    #[tokio::test]
    async fn run_acli_checked_does_not_block_allowlisted_argv() {
        let tmp = tempfile::tempdir().unwrap();
        // Allowlisted argv reaches `run_command` (may succeed, fail auth, or fail if `acli` is absent).
        let res = run_acli_checked(
            &["jira", "auth", "status"],
            &[],
            tmp.path(),
            CancellationToken::new(),
        )
        .await;

        if let Err(e) = res {
            let s = e.to_string();
            assert!(
                !s.contains("blocked disallowed acli"),
                "allowlisted argv must not be rejected by allowlist: {s}"
            );
        }
    }
}
