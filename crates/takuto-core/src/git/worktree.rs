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
    let key_lower = ticket_key.to_lowercase();
    if run_index <= 1 {
        format!("{prefix}/{key_lower}")
    } else {
        format!("{prefix}/{key_lower}-{run_index}")
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
}
