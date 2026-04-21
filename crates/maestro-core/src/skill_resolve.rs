// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Skill resolution for workflow steps.
//!
//! Skills are directories containing a `SKILL.md` with optional YAML frontmatter.
//!
//! For **Claude** (`--bare` mode): skills are read, args substituted, and the result
//! is passed via `--system-prompt`.  Claude Code's `--print` mode does not support
//! native `/skill` invocations, so Maestro handles skill content injection.
//!
//! For **Cursor**: skills work natively in interactive sessions, so we build
//! `/skill-name arg1 arg2` invocation lines to prepend to the prompt.

use std::path::PathBuf;

use tracing::warn;

use crate::config::SkillRef;

/// Search for `<name>/SKILL.md` in the given directories. Return the body (frontmatter stripped).
pub async fn find_and_read_skill(name: &str, search_paths: &[PathBuf]) -> Option<String> {
    for dir in search_paths {
        let path = dir.join(name).join("SKILL.md");
        if let Ok(content) = tokio::fs::read_to_string(&path).await {
            return Some(strip_frontmatter(&content));
        }
    }
    None
}

/// Strip YAML frontmatter (between first `---` and second `---`) from the content.
pub fn strip_frontmatter(content: &str) -> String {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return content.to_string();
    }
    let after_first = &trimmed[3..];
    if let Some(end) = after_first.find("\n---") {
        let body = &after_first[end + 4..];
        body.trim_start_matches('\n').to_string()
    } else {
        content.to_string()
    }
}

/// Substitute skill arguments into the body text.
///
/// Replaces `$ARGUMENTS` with all args joined by space, and `$1`, `$2`, … with
/// positional values.
pub fn substitute_skill_args(body: &str, args: &[String]) -> String {
    let joined = args.join(" ");
    let mut result = body.replace("$ARGUMENTS", &joined);
    for (i, arg) in args.iter().enumerate() {
        result = result.replace(&format!("${}", i + 1), arg);
    }
    result
}

/// Build a `--system-prompt` value for Claude from the step's skill references.
///
/// Reads each skill's SKILL.md, strips frontmatter, substitutes arguments, and
/// concatenates them.  Returns `None` if no skills are configured or none are found.
pub async fn build_system_prompt(skills: &[SkillRef], search_paths: &[PathBuf]) -> Option<String> {
    if skills.is_empty() {
        return None;
    }

    let mut parts: Vec<String> = Vec::new();
    for skill in skills {
        if let Some(body) = find_and_read_skill(&skill.name, search_paths).await {
            let substituted = if skill.args.is_empty() {
                body
            } else {
                substitute_skill_args(&body, &skill.args)
            };
            parts.push(substituted);
        } else {
            warn!(skill = %skill.name, "Skill not found in search paths — skipping");
        }
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n\n---\n\n"))
    }
}

/// Build `/skill-name arg1 arg2` invocation lines for Cursor (native skill support).
pub fn build_cursor_skill_invocations(skills: &[SkillRef]) -> String {
    skills
        .iter()
        .map(|s| {
            if s.args.is_empty() {
                format!("/{}", s.name)
            } else {
                format!("/{} {}", s.name, s.args.join(" "))
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
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
    fn strip_frontmatter_removes_yaml() {
        let input = "---\nname: test\ndescription: hello\n---\n\n# Body\n\nContent here.";
        assert_eq!(strip_frontmatter(input), "# Body\n\nContent here.");
    }

    #[test]
    fn strip_frontmatter_no_frontmatter() {
        let input = "# Just content\n\nNo frontmatter.";
        assert_eq!(strip_frontmatter(input), input);
    }

    #[test]
    fn substitute_args_positional() {
        let body = "Fix issue $1 with priority $2.\n\nAll args: $ARGUMENTS";
        let args = vec!["NERO-202".into(), "high".into()];
        let result = substitute_skill_args(body, &args);
        assert_eq!(
            result,
            "Fix issue NERO-202 with priority high.\n\nAll args: NERO-202 high"
        );
    }

    #[test]
    fn substitute_args_empty() {
        let body = "No args here.";
        let result = substitute_skill_args(body, &[]);
        assert_eq!(result, "No args here.");
    }

    #[tokio::test]
    async fn build_system_prompt_single_skill() {
        let tmp = TempDir::new().unwrap();
        write_skill(
            tmp.path(),
            "caveman",
            "---\nname: caveman\n---\n\n# Caveman Mode\n\nTalk like caveman.",
        );
        let paths = vec![tmp.path().to_path_buf()];
        let skills = vec![SkillRef {
            name: "caveman".into(),
            args: vec!["ultra".into()],
        }];

        let result = build_system_prompt(&skills, &paths).await;
        assert!(result.is_some());
        let content = result.unwrap();
        assert!(content.contains("# Caveman Mode"));
        assert!(!content.contains("---\nname"));
    }

    #[tokio::test]
    async fn build_system_prompt_missing_skill() {
        let paths = vec![PathBuf::from("/nonexistent")];
        let skills = vec![SkillRef {
            name: "unknown".into(),
            args: Vec::new(),
        }];

        let result = build_system_prompt(&skills, &paths).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn build_system_prompt_multiple_skills() {
        let tmp = TempDir::new().unwrap();
        write_skill(tmp.path(), "skill-a", "---\nname: a\n---\n\nContent A.");
        write_skill(tmp.path(), "skill-b", "---\nname: b\n---\n\nContent B.");
        let paths = vec![tmp.path().to_path_buf()];
        let skills = vec![
            SkillRef {
                name: "skill-a".into(),
                args: Vec::new(),
            },
            SkillRef {
                name: "skill-b".into(),
                args: Vec::new(),
            },
        ];

        let result = build_system_prompt(&skills, &paths).await.unwrap();
        assert!(result.contains("Content A."));
        assert!(result.contains("---"));
        assert!(result.contains("Content B."));
    }

    #[test]
    fn cursor_skill_invocations() {
        let skills = vec![
            SkillRef {
                name: "caveman".into(),
                args: vec!["ultra".into()],
            },
            SkillRef {
                name: "address-ticket".into(),
                args: Vec::new(),
            },
        ];
        let result = build_cursor_skill_invocations(&skills);
        assert_eq!(result, "/caveman ultra\n/address-ticket");
    }

    #[tokio::test]
    async fn build_system_prompt_empty_skills() {
        let result = build_system_prompt(&[], &[]).await;
        assert!(result.is_none());
    }
}
