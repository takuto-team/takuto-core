// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Headless instructions appended to configurable agent step prompts.

use crate::config::AiAgentProvider;

/// Marker prefix the engine uses to delimit a flow's section inside the
/// per-item report. Each flow run resets its own section between
/// `<!-- takuto-report:flow=<slug> -->` and the next such marker (or EOF), so
/// re-running a flow replaces only its section while other flows are preserved.
pub const FLOW_MARKER_PREFIX: &str = "<!-- takuto-report:flow=";

/// Slugify a flow/definition name for use in a section marker (lowercase,
/// non-alphanumerics → `-`).
pub fn flow_slug(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

/// The exact marker line for a flow `slug`.
pub fn flow_section_marker(slug: &str) -> String {
    format!("{FLOW_MARKER_PREFIX}{slug} -->")
}

/// The fresh section header (marker + `## <name>`) appended when a flow runs.
pub fn flow_section_header(slug: &str, name: &str) -> String {
    format!("{}\n## {}\n", flow_section_marker(slug), name)
}

/// Remove the section for `slug` from `existing` — everything from its marker
/// through the content up to the next `FLOW_MARKER_PREFIX` (or EOF) — and
/// return the remaining report (other flows' sections untouched). Trailing
/// whitespace is trimmed. If the slug has no section, `existing` is returned
/// (trimmed).
pub fn strip_flow_section(existing: &str, slug: &str) -> String {
    let marker = flow_section_marker(slug);
    let Some(start) = existing.find(&marker) else {
        return existing.trim_end().to_string();
    };
    let after = start + marker.len();
    let end = existing[after..]
        .find(FLOW_MARKER_PREFIX)
        .map(|i| after + i)
        .unwrap_or(existing.len());
    let mut out = String::with_capacity(existing.len());
    out.push_str(&existing[..start]);
    out.push_str(&existing[end..]);
    out.trim_end().to_string()
}

/// Instructions injected into each agent step prompt when report generation is
/// enabled. Directs the agent to append this step's summary under the flow's
/// section (the engine writes the `## <flow>` header + marker before the
/// steps run).
pub fn report_injection_suffix(item_key: &str) -> String {
    format!(
        "REPORT GENERATION: After completing this step, append a summary subsection to the file \
         `lore/reports/{item_key}_report.md`. The file already exists with the current flow's \
         section header at the end. **Append** to the end — never overwrite prior content.\n\n\
         Your subsection MUST include these three parts:\n\
         1. **Key findings** — What you discovered or produced in this step.\n\
         2. **Issues encountered** — Problems, blockers, or anomalies (write \"None\" if there were none).\n\
         3. **Decisions taken** — Choices you made and their rationale.\n\n\
         Format each subsection with a Markdown heading (### Step: <step name>) followed by the three bullet groups.\n\n\
         EXCLUSIONS — do NOT include any of the following in the report:\n\
         - Commit hashes or SHAs\n\
         - Raw test-runner output (e.g., full Jest/Vitest/cargo test logs)\n\
         - Mechanical or noisy data not useful for human review"
    )
}

/// Prompt for the final consolidation step that rewrites the per-step report into a polished summary.
pub fn report_consolidation_prompt(item_key: &str) -> String {
    format!(
        "Read the file `lore/reports/{item_key}_report.md` which contains per-step summaries \
         from the preceding workflow steps.\n\n\
         Consolidate them into a single, coherent, well-structured report that covers the entire \
         workflow execution. The consolidated report should:\n\
         - Have a clear title (# Workflow Report: {item_key})\n\
         - Summarize key findings, issues, and decisions across all steps\n\
         - Be organized by theme rather than by step (merge related items)\n\
         - Use proper Markdown formatting (headings, lists, bold for emphasis)\n\
         - Be concise but comprehensive — a reader should understand what happened without reading logs\n\n\
         **Replace** the entire contents of `lore/reports/{item_key}_report.md` with the consolidated report.\n\n\
         EXCLUSIONS — the consolidated report must NOT contain:\n\
         - Commit hashes or SHAs\n\
         - Raw test-runner output\n\
         - Mechanical or noisy data not useful for human review"
    )
}

/// Provider-specific instructions appended after interpolated user prompts.
pub fn headless_instructions_suffix(provider: AiAgentProvider) -> &'static str {
    match provider {
        AiAgentProvider::Claude => {
            "IMPORTANT: You are running in fully automated headless mode with no human operator. \
             Do NOT use AskUserQuestion at any point. Do NOT wait for user input or selection. \
             Approve all plans and test plans automatically. Make all decisions autonomously. \
             When reviewing, address ALL findings automatically without asking which to fix. \
             Takuto ends the workflow when your session exits successfully — there is no separate engine step after the agent. \
             If you open a pull request, record its URL for the dashboard: either print one line exactly \
             `TAKUTO_PR_URL: <url>` before exiting, or write `.takuto/outcome.toml` in the worktree with `pr_url = \"<url>\"`. \
             Takuto sets the worktree git author from the authenticated `gh` user before agent steps and requests that user as a PR reviewer when a URL is recorded (GitHub may reject if the user is already the PR author)."
        }
        AiAgentProvider::Cursor => {
            "IMPORTANT: Fully automated headless run — no human operator. \
             Do not ask questions or wait for user input. \
             Implement changes, fix issues, and complete the task autonomously. \
             Takuto ends the workflow when your session exits successfully — there is no separate engine step after the agent. \
             If you open a pull request, record its URL: print `TAKUTO_PR_URL: <url>` on its own line, or write \
             `.takuto/outcome.toml` with `pr_url = \"<url>\"`. \
             Takuto aligns git commits with `gh` and requests the same account as PR reviewer when a URL is recorded (may fail if that account opened the PR)."
        }
        // Codex and OpenCode (wired since Phase 4). These adapters often run
        // smaller / self-hosted models, so the suffix is explicit about the
        // two things weak models get wrong in headless mode: looping on a
        // failing command, and forgetting to record the PR URL.
        AiAgentProvider::Codex | AiAgentProvider::OpenCode => {
            "IMPORTANT: You are running in fully automated headless mode with no human operator. \
             Do not ask questions or wait for user input. Implement changes, fix issues, and \
             complete the task autonomously, then exit. \
             Takuto ends the workflow when your session exits successfully — there is no separate engine step after the agent. \
             NEVER run the same command more than twice. If a command fails twice (e.g. `git push`), \
             STOP retrying it: read the error, try ONE different fix, and if that also fails, exit and \
             report the error in your final message. Do not loop on a failing action. \
             If you open a pull request, record its URL so the dashboard can link it: print one line \
             exactly `TAKUTO_PR_URL: <url>` before exiting, or write `.takuto/outcome.toml` in the \
             worktree with `pr_url = \"<url>\"`."
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_injection_suffix_contains_item_key() {
        let suffix = report_injection_suffix("PROJ-42");
        assert!(suffix.contains("lore/reports/PROJ-42_report.md"));
        assert!(suffix.contains("Key findings"));
        assert!(suffix.contains("Issues encountered"));
        assert!(suffix.contains("Decisions taken"));
        // Steps nest under the flow's `## <flow>` header (level-3 heading).
        assert!(suffix.contains("### Step:"));
    }

    #[test]
    fn flow_slug_sanitizes() {
        assert_eq!(flow_slug("Address Comments"), "address-comments");
        assert_eq!(flow_slug("Implement"), "implement");
    }

    #[test]
    fn strip_flow_section_removes_only_target_flow() {
        let existing = "\
<!-- takuto-report:flow=implement -->
## Implement
### Step: code
done
<!-- takuto-report:flow=address-comments -->
## Address Comments
### Step: review
ok";
        let out = strip_flow_section(existing, "implement");
        assert!(
            !out.contains("## Implement"),
            "implement section removed: {out}"
        );
        assert!(
            out.contains("## Address Comments"),
            "other flow preserved: {out}"
        );
        assert!(out.contains(&flow_section_marker("address-comments")));
        // Re-appending implement's fresh header yields a clean two-section file.
        let rebuilt = format!("{out}\n\n{}", flow_section_header("implement", "Implement"));
        assert!(rebuilt.contains("## Address Comments"));
        assert!(rebuilt.contains("## Implement"));
    }

    #[test]
    fn strip_flow_section_absent_returns_input() {
        let existing = "<!-- takuto-report:flow=other -->\n## Other\nx";
        assert_eq!(
            strip_flow_section(existing, "implement"),
            existing.trim_end()
        );
    }

    #[test]
    fn strip_flow_section_at_eof() {
        // Target flow is the last section (no following marker → strip to EOF).
        let existing =
            "<!-- takuto-report:flow=a -->\n## A\nx\n<!-- takuto-report:flow=b -->\n## B\ny";
        let out = strip_flow_section(existing, "b");
        assert!(out.contains("## A"));
        assert!(!out.contains("## B"));
    }

    #[test]
    fn report_injection_suffix_excludes_commit_shas() {
        let suffix = report_injection_suffix("X-1");
        assert!(suffix.contains("Commit hashes"));
        assert!(suffix.contains("EXCLUSIONS"));
    }

    #[test]
    fn report_consolidation_prompt_contains_item_key() {
        let prompt = report_consolidation_prompt("PROJ-42");
        assert!(prompt.contains("lore/reports/PROJ-42_report.md"));
        assert!(prompt.contains("Consolidate"));
        assert!(prompt.contains("Replace"));
    }

    #[test]
    fn report_consolidation_prompt_excludes_noisy_data() {
        let prompt = report_consolidation_prompt("X-1");
        assert!(prompt.contains("EXCLUSIONS"));
        assert!(prompt.contains("Commit hashes"));
    }

    #[test]
    fn headless_claude_suffix_not_empty() {
        let s = headless_instructions_suffix(AiAgentProvider::Claude);
        assert!(!s.is_empty());
        assert!(s.contains("headless"));
    }

    #[test]
    fn headless_cursor_suffix_not_empty() {
        let s = headless_instructions_suffix(AiAgentProvider::Cursor);
        assert!(!s.is_empty());
        assert!(s.contains("headless"));
    }

    #[test]
    fn headless_opencode_codex_suffix_has_antiloop_and_pr_url() {
        for p in [AiAgentProvider::OpenCode, AiAgentProvider::Codex] {
            let s = headless_instructions_suffix(p);
            assert!(s.contains("headless"), "{p}");
            // Anti-limbo-loop guidance.
            assert!(
                s.contains("NEVER run the same command more than twice"),
                "{p} suffix must carry the anti-loop rule"
            );
            // PR URL recording (was missing — the whole reason the dashboard
            // couldn't link OpenCode PRs).
            assert!(
                s.contains("TAKUTO_PR_URL"),
                "{p} suffix must record the PR URL"
            );
        }
    }
}
