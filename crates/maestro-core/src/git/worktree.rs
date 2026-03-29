pub fn branch_name_for_ticket(ticket_key: &str, item_type: &str) -> String {
    let prefix = match item_type.to_lowercase().as_str() {
        "bug" => "fix",
        _ => "feat",
    };
    let key_lower = ticket_key.to_lowercase();
    format!("{prefix}/{key_lower}")
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
        assert_eq!(
            branch_name_for_ticket("CORE-789", "Story"),
            "feat/core-789"
        );
    }
}
