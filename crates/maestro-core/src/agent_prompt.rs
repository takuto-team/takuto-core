//! Headless instructions appended to configurable agent step prompts.

use crate::config::AiAgentProvider;

/// Provider-specific instructions appended after interpolated user prompts.
pub fn headless_instructions_suffix(provider: AiAgentProvider) -> &'static str {
    match provider {
        AiAgentProvider::Claude => {
            "IMPORTANT: You are running in fully automated headless mode with no human operator. \
             Do NOT use AskUserQuestion at any point. Do NOT wait for user input or selection. \
             Approve all plans and test plans automatically. Make all decisions autonomously. \
             When reviewing, address ALL findings automatically without asking which to fix. \
             Maestro ends the workflow when your session exits successfully — there is no separate engine step after the agent. \
             If you open a pull request, record its URL for the dashboard: either print one line exactly \
             `MAESTRO_PR_URL: <url>` before exiting, or write `.maestro/outcome.toml` in the worktree with `pr_url = \"<url>\"`. \
             Maestro sets the worktree git author from the authenticated `gh` user before agent steps and requests that user as a PR reviewer when a URL is recorded (GitHub may reject if the user is already the PR author)."
        }
        AiAgentProvider::Cursor => {
            "IMPORTANT: Fully automated headless run — no human operator. \
             Do not ask questions or wait for user input. \
             Implement changes, fix issues, and complete the task autonomously. \
             Maestro ends the workflow when your session exits successfully — there is no separate engine step after the agent. \
             If you open a pull request, record its URL: print `MAESTRO_PR_URL: <url>` on its own line, or write \
             `.maestro/outcome.toml` with `pr_url = \"<url>\"`. \
             Maestro aligns git commits with `gh` and requests the same account as PR reviewer when a URL is recorded (may fail if that account opened the PR)."
        }
    }
}
