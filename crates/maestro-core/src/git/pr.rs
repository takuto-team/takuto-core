pub fn pr_title(ticket_key: &str, ticket_summary: &str, item_type: &str) -> String {
    let prefix = match item_type.to_lowercase().as_str() {
        "bug" => "fix",
        _ => "feat",
    };
    format!("{prefix}({ticket_key}): {ticket_summary}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pr_title_task() {
        assert_eq!(
            pr_title("PROJ-123", "Add login page", "Task"),
            "feat(PROJ-123): Add login page"
        );
    }

    #[test]
    fn test_pr_title_bug() {
        assert_eq!(
            pr_title("PROJ-456", "Fix crash on empty input", "Bug"),
            "fix(PROJ-456): Fix crash on empty input"
        );
    }
}
