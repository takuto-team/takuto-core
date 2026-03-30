use std::path::Path;

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::config::{AgentConfig, AiAgentProvider};
use crate::error::{MaestroError, Result};
use crate::process::ProcessHandle;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PmVerdict {
    Approved,
    Rejected { reasons: Vec<String> },
}

pub struct PmAgent {
    pub ticket_description: String,
    pub acceptance_criteria: Vec<String>,
}

impl PmAgent {
    pub fn new(ticket_description: String, acceptance_criteria: Vec<String>) -> Self {
        Self {
            ticket_description,
            acceptance_criteria,
        }
    }

    pub async fn validate_plan(
        &self,
        plan: &str,
        worktree: &Path,
        cancel_token: CancellationToken,
        agent_cfg: &AgentConfig,
        model: &str,
    ) -> Result<PmVerdict> {
        info!(
            provider = ?agent_cfg.provider,
            "PM agent validating plan against acceptance criteria"
        );

        let criteria_text = if self.acceptance_criteria.is_empty() {
            "No explicit acceptance criteria provided.".to_string()
        } else {
            self.acceptance_criteria
                .iter()
                .enumerate()
                .map(|(i, c)| format!("{}. {}", i + 1, c))
                .collect::<Vec<_>>()
                .join("\n")
        };

        let prompt = format!(
            r#"You are a PM validating a development plan against ticket requirements.

## Ticket Description
{description}

## Acceptance Criteria
{criteria}

## Proposed Plan
{plan}

## Instructions
Evaluate whether this plan adequately addresses all the requirements and acceptance criteria.
Respond with EXACTLY one of:
- "APPROVED" if the plan covers all requirements
- "REJECTED: <reason1>; <reason2>" if the plan is missing key requirements

Be pragmatic — approve plans that reasonably cover the requirements even if they don't explicitly mention every detail."#,
            description = self.ticket_description,
            criteria = criteria_text,
            plan = plan,
        );

        let output = match agent_cfg.provider {
            AiAgentProvider::Claude => {
                let handle = ProcessHandle::spawn(
                    "claude",
                    &[
                        "--allow-dangerously-skip-permissions",
                        "--print",
                        "-p",
                        &prompt,
                    ],
                    worktree,
                    cancel_token,
                )
                .await
                .map_err(|e| MaestroError::Claude(format!("Failed to spawn PM agent: {e}")))?;

                handle.wait_with_timeout(120).await.map_err(|e| {
                    MaestroError::Claude(format!("PM agent failed: {e}"))
                })?
            }
            AiAgentProvider::Cursor => {
                let workspace = worktree.to_str().ok_or_else(|| {
                    MaestroError::AiAgent("Worktree path is not valid UTF-8".to_string())
                })?;
                let mut owned: Vec<String> = vec![
                    "-p".to_string(),
                    prompt.clone(),
                    "--output-format".to_string(),
                    "text".to_string(),
                    "--trust".to_string(),
                    "--force".to_string(),
                    "--approve-mcps".to_string(),
                    "--sandbox".to_string(),
                    "disabled".to_string(),
                    "--workspace".to_string(),
                    workspace.to_string(),
                ];
                if !model.trim().is_empty() {
                    owned.push("--model".to_string());
                    owned.push(model.to_string());
                }
                let arg_refs: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();
                let handle = ProcessHandle::spawn(&agent_cfg.cursor_cli, &arg_refs, worktree, cancel_token)
                    .await
                    .map_err(|e| MaestroError::AiAgent(format!("Failed to spawn PM agent (Cursor): {e}")))?;
                handle.wait_with_timeout(120).await.map_err(|e| {
                    MaestroError::AiAgent(format!("PM agent (Cursor) failed: {e}"))
                })?
            }
        };

        let response = output.stdout.trim().to_uppercase();

        if response.starts_with("APPROVED") {
            info!("PM agent approved the plan");
            Ok(PmVerdict::Approved)
        } else if response.starts_with("REJECTED") {
            let reasons_text = response
                .strip_prefix("REJECTED:")
                .or_else(|| response.strip_prefix("REJECTED"))
                .unwrap_or("")
                .trim();
            let reasons: Vec<String> = reasons_text
                .split(';')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            warn!(?reasons, "PM agent rejected the plan");
            Ok(PmVerdict::Rejected { reasons })
        } else {
            // Default to rejected if we can't parse the response
            warn!(
                response = %output.stdout,
                "Could not parse PM agent response, defaulting to rejected"
            );
            Ok(PmVerdict::Rejected {
                reasons: vec!["PM agent response could not be parsed".to_string()],
            })
        }
    }
}
