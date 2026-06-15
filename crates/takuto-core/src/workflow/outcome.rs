// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Optional workflow outcome written by agent steps (e.g. PR URL).

use std::path::Path;

use serde::Deserialize;
use tracing::warn;

/// Parsed `.takuto/outcome.toml` in the worktree.
#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
pub struct WorkflowOutcome {
    #[serde(default)]
    pub pr_url: Option<String>,
}

/// Reads `{worktree}/.takuto/outcome.toml` if present and valid.
pub fn read_workflow_outcome(worktree: &Path) -> Option<WorkflowOutcome> {
    let path = worktree.join(".takuto").join("outcome.toml");
    let raw = std::fs::read_to_string(&path).ok()?;
    if raw.trim().is_empty() {
        return None;
    }
    match toml::from_str::<WorkflowOutcome>(&raw) {
        Ok(o) => Some(o),
        Err(e) => {
            warn!(path = %path.display(), error = %e, "Invalid .takuto/outcome.toml");
            None
        }
    }
}

/// Looks for a line `TAKUTO_PR_URL: <url>` in agent session output.
pub fn pr_url_from_agent_output(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("TAKUTO_PR_URL:")?;
        let u = rest.trim();
        if u.is_empty() {
            None
        } else {
            Some(u.to_string())
        }
    })
}

/// Prefer TOML file, then stdout marker.
pub fn resolve_pr_url(worktree: &Path, last_agent_output: Option<&str>) -> Option<String> {
    if let Some(o) = read_workflow_outcome(worktree)
        && let Some(u) = o
            .pr_url
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
    {
        return Some(u.to_string());
    }
    last_agent_output.and_then(pr_url_from_agent_output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pr_url_from_output_finds_marker() {
        let s = "done\nTAKUTO_PR_URL: https://github.com/a/b/pull/1\n";
        assert_eq!(
            pr_url_from_agent_output(s).as_deref(),
            Some("https://github.com/a/b/pull/1")
        );
    }

    #[test]
    fn read_outcome_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".takuto")).unwrap();
        std::fs::write(
            dir.path().join(".takuto").join("outcome.toml"),
            r#"pr_url = "https://x/y/1"
"#,
        )
        .unwrap();
        let o = read_workflow_outcome(dir.path()).unwrap();
        assert_eq!(o.pr_url.as_deref(), Some("https://x/y/1"));
    }
}
