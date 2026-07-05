// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

pub fn branch_name_for_ticket(ticket_key: &str, item_type: &str) -> String {
    branch_name_for_ticket_run(ticket_key, item_type, 1)
}

/// Branch name for the `run_index`-th run of a ticket (1-based).
///
/// The first run keeps the canonical name (`feat/gh-4`); every re-add gets a
/// unique suffix (`feat/gh-4-2`, `feat/gh-4-3`, …) so the agent's `gh pr create`
/// targets a fresh head and opens a brand-new PR instead of returning the open
/// PR from a previous run on the same deterministic branch.
pub fn branch_name_for_ticket_run(ticket_key: &str, item_type: &str, run_index: u32) -> String {
    let prefix = match item_type.to_lowercase().as_str() {
        "bug" => "fix",
        _ => "feat",
    };
    let key_slug = slugify_ref_component(ticket_key);
    if run_index <= 1 {
        format!("{prefix}/{key_slug}")
    } else {
        format!("{prefix}/{key_slug}-{run_index}")
    }
}

/// Slugify a ticket key into a single git-ref-safe path component: lowercase,
/// keep `[a-z0-9_]`, map every other char (spaces, `.`, `/`, punctuation) to a
/// single `-`, collapse runs of `-`, and trim leading/trailing `-`. A key with
/// spaces (e.g. a locally-created ticket titled "very simple test") would
/// otherwise yield `feat/very simple test` — an invalid ref that word-splits
/// the `git worktree add` argv. Empty result falls back to `item`.
fn slugify_ref_component(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut prev_dash = false;
    for c in raw.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if c == '_' {
            out.push('_');
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "item".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_branch_name_task() {
        assert_eq!(branch_name_for_ticket("PROJ-123", "Task"), "feat/proj-123");
    }

    #[test]
    fn test_branch_name_bug() {
        assert_eq!(branch_name_for_ticket("PROJ-456", "Bug"), "fix/proj-456");
    }

    #[test]
    fn test_branch_name_story() {
        assert_eq!(branch_name_for_ticket("CORE-789", "Story"), "feat/core-789");
    }

    #[test]
    fn test_branch_name_first_run_has_no_suffix() {
        assert_eq!(
            branch_name_for_ticket_run("GH-4", "Task", 1),
            "feat/gh-4",
            "first run must keep the canonical branch name"
        );
    }

    #[test]
    fn test_branch_name_re_add_gets_run_suffix() {
        assert_eq!(branch_name_for_ticket_run("GH-4", "Task", 2), "feat/gh-4-2");
        assert_eq!(branch_name_for_ticket_run("GH-4", "Task", 3), "feat/gh-4-3");
        assert_eq!(branch_name_for_ticket_run("GH-9", "Bug", 2), "fix/gh-9-2");
    }

    #[test]
    fn test_branch_name_slugifies_spaces_and_punctuation() {
        // A locally-created ticket keyed by its title must not produce a ref
        // with spaces (the reported `git worktree add` usage error).
        assert_eq!(
            branch_name_for_ticket("very simple test", "Task"),
            "feat/very-simple-test"
        );
        assert_eq!(
            branch_name_for_ticket("Fix: the thing (again)!", "Bug"),
            "fix/fix-the-thing-again"
        );
        // Run suffix still applies on top of the slug.
        assert_eq!(
            branch_name_for_ticket_run("very simple test", "Task", 2),
            "feat/very-simple-test-2"
        );
    }

    #[test]
    fn test_branch_name_empty_key_falls_back() {
        assert_eq!(branch_name_for_ticket("   ", "Task"), "feat/item");
        assert_eq!(branch_name_for_ticket("!!!", "Bug"), "fix/item");
    }

    #[test]
    fn slugify_ref_component_keeps_valid_keys_stable() {
        assert_eq!(slugify_ref_component("PROJ-123"), "proj-123");
        assert_eq!(slugify_ref_component("GH-4"), "gh-4");
        assert_eq!(slugify_ref_component("a_b-C1"), "a_b-c1");
    }
}
