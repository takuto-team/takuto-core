//! Resolve `/skill-name args` invocations in workflow prompts by reading SKILL.md files.
//!
//! Skills are directories containing a `SKILL.md` with optional YAML frontmatter.
//! The engine replaces each invocation line with the skill body, appending
//! `ARGUMENTS: <args>` when arguments are present.

use std::path::PathBuf;

use tracing::warn;

/// Resolve all `/skill-name [args]` lines in `prompt`.
///
/// Each line whose trimmed form starts with `/` followed by a lowercase skill name
/// (`[a-z][a-z0-9-]*`) is looked up under `search_paths` as `<dir>/<name>/SKILL.md`.
/// First match wins. Unresolved invocations are left as-is with a warning.
pub async fn resolve_skill_invocations(prompt: &str, search_paths: &[PathBuf]) -> String {
    let mut out_lines: Vec<String> = Vec::new();

    for line in prompt.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix('/') {
            if let Some((name, args)) = parse_skill_invocation(rest) {
                if let Some(body) = find_and_read_skill(&name, search_paths).await {
                    if args.is_empty() {
                        out_lines.push(body);
                    } else {
                        out_lines.push(format!("{body}\n\nARGUMENTS: {args}"));
                    }
                    continue;
                }
                warn!(skill = %name, "Skill not found in search paths — leaving invocation as-is");
            }
        }
        out_lines.push(line.to_string());
    }

    out_lines.join("\n")
}

/// Parse `rest` (after the leading `/`) into `(skill_name, args)`.
/// Skill names: `[a-z][a-z0-9-]*` (lowercase, hyphens allowed, no underscores).
fn parse_skill_invocation(rest: &str) -> Option<(String, String)> {
    let mut chars = rest.chars().peekable();

    // First char must be lowercase letter.
    let first = chars.peek()?;
    if !first.is_ascii_lowercase() {
        return None;
    }

    let mut name = String::new();
    for ch in chars.by_ref() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' {
            name.push(ch);
        } else if ch.is_ascii_whitespace() {
            break;
        } else {
            // Invalid character for a skill name (e.g. uppercase, underscore, dot).
            return None;
        }
    }

    if name.is_empty() {
        return None;
    }

    let args: String = chars.collect::<String>().trim().to_string();
    Some((name, args))
}

/// Search for `<name>/SKILL.md` in the given directories. Return the body (frontmatter stripped).
async fn find_and_read_skill(name: &str, search_paths: &[PathBuf]) -> Option<String> {
    for dir in search_paths {
        let path = dir.join(name).join("SKILL.md");
        if let Ok(content) = tokio::fs::read_to_string(&path).await {
            return Some(strip_frontmatter(&content));
        }
    }
    None
}

/// Strip YAML frontmatter (between first `---` and second `---`) from the content.
fn strip_frontmatter(content: &str) -> String {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return content.to_string();
    }
    // Find end of frontmatter (second `---`).
    let after_first = &trimmed[3..];
    if let Some(end) = after_first.find("\n---") {
        let body = &after_first[end + 4..]; // skip the "\n---"
        body.trim_start_matches('\n').to_string()
    } else {
        // Malformed frontmatter — return as-is.
        content.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::Path;
    use tempfile::TempDir;

    fn write_skill(dir: &Path, name: &str, content: &str) {
        let skill_dir = dir.join(name);
        std::fs::create_dir_all(&skill_dir).unwrap();
        let mut f = std::fs::File::create(skill_dir.join("SKILL.md")).unwrap();
        f.write_all(content.as_bytes()).unwrap();
    }

    #[test]
    fn parse_skill_name_and_args() {
        assert_eq!(
            parse_skill_invocation("caveman ultra"),
            Some(("caveman".into(), "ultra".into()))
        );
        assert_eq!(
            parse_skill_invocation("create-pr --no-draft"),
            Some(("create-pr".into(), "--no-draft".into()))
        );
        assert_eq!(
            parse_skill_invocation("review-changes"),
            Some(("review-changes".into(), String::new()))
        );
    }

    #[test]
    fn parse_rejects_invalid() {
        // Uppercase
        assert_eq!(parse_skill_invocation("Caveman"), None);
        // Starts with digit
        assert_eq!(parse_skill_invocation("1skill"), None);
        // Path-like
        assert_eq!(parse_skill_invocation("foo/bar"), None);
    }

    #[test]
    fn strip_frontmatter_removes_yaml() {
        let input = "---\nname: test\ndescription: hello\n---\n\n# Body\n\nContent here.";
        assert_eq!(strip_frontmatter(input), "# Body\n\nContent here.");
    }

    #[test]
    fn strip_frontmatter_no_frontmatter() {
        let input = "# Just content\n\nNo frontmatter.";
        assert_eq!(strip_frontmatter(input), input);
    }

    #[tokio::test]
    async fn resolve_replaces_skill_invocation() {
        let tmp = TempDir::new().unwrap();
        write_skill(
            tmp.path(),
            "caveman",
            "---\nname: caveman\ndescription: compress\n---\n\n# Caveman Mode\n\nTalk like caveman.",
        );
        let paths = vec![tmp.path().to_path_buf()];

        let result = resolve_skill_invocations("/caveman ultra", &paths).await;
        assert!(result.contains("# Caveman Mode"));
        assert!(result.contains("Talk like caveman."));
        assert!(result.contains("ARGUMENTS: ultra"));
        assert!(!result.contains("/caveman"));
    }

    #[tokio::test]
    async fn resolve_no_args() {
        let tmp = TempDir::new().unwrap();
        write_skill(
            tmp.path(),
            "cleanup",
            "---\nname: cleanup\n---\n\n# Cleanup\n\nRun cleanup.",
        );
        let paths = vec![tmp.path().to_path_buf()];

        let result = resolve_skill_invocations("/cleanup", &paths).await;
        assert!(result.contains("# Cleanup"));
        assert!(!result.contains("ARGUMENTS"));
    }

    #[tokio::test]
    async fn resolve_mixed_prompt() {
        let tmp = TempDir::new().unwrap();
        write_skill(
            tmp.path(),
            "review-changes",
            "---\nname: review-changes\n---\n\n# Review\n\nReview all changes.",
        );
        let paths = vec![tmp.path().to_path_buf()];

        let prompt = "/review-changes\n\nFix all confirmed findings.\nDo not create a PR.";
        let result = resolve_skill_invocations(prompt, &paths).await;
        assert!(result.contains("# Review"));
        assert!(result.contains("Fix all confirmed findings."));
        assert!(result.contains("Do not create a PR."));
    }

    #[tokio::test]
    async fn resolve_unknown_skill_left_as_is() {
        let paths = vec![PathBuf::from("/nonexistent")];
        let prompt = "/unknown-skill args here";
        let result = resolve_skill_invocations(prompt, &paths).await;
        assert_eq!(result, prompt);
    }

    #[tokio::test]
    async fn resolve_search_order_first_wins() {
        let dir1 = TempDir::new().unwrap();
        let dir2 = TempDir::new().unwrap();
        write_skill(dir1.path(), "test-skill", "---\nname: test-skill\n---\n\nFrom dir1.");
        write_skill(dir2.path(), "test-skill", "---\nname: test-skill\n---\n\nFrom dir2.");
        let paths = vec![dir1.path().to_path_buf(), dir2.path().to_path_buf()];

        let result = resolve_skill_invocations("/test-skill", &paths).await;
        assert!(result.contains("From dir1."));
        assert!(!result.contains("From dir2."));
    }

    #[tokio::test]
    async fn resolve_preserves_non_skill_slashes() {
        let paths = vec![PathBuf::from("/nonexistent")];
        // Paths, URLs, etc. should not be treated as skill invocations
        let prompt = "Run /usr/bin/test and check https://example.com/path";
        let result = resolve_skill_invocations(prompt, &paths).await;
        assert_eq!(result, prompt);
    }
}
