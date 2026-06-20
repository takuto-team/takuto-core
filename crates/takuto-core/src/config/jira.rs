// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Jira site, polling, and prompt-mode policy for linked issues.

use serde::{Deserialize, Serialize};

/// How linked Jira issues are included in `{ticket_context}` for agent prompts.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum LinkedItemsPromptMode {
    /// Key, summary, status, link type, and description (subject to byte caps).
    #[default]
    Full,
    /// Key, summary, status, and link type only (descriptions omitted).
    SummaryOnly,
    /// Linked issues are not included in the context string.
    Omit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JiraConfig {
    #[serde(default)]
    pub project_keys: Vec<String>,
    #[serde(default = "default_item_types")]
    pub item_types: Vec<String>,
    #[serde(default)]
    pub jql_filter: String,
    #[serde(default)]
    pub site: String,
    #[serde(default)]
    pub email: String,
    /// Status name for **Mark as Done** (Jira transition target). Must match your workflow.
    #[serde(default = "default_jira_done_status")]
    pub done_status: String,
    /// How linked issues appear in agent prompts (`{ticket_context}`).
    #[serde(default)]
    pub linked_items_in_prompt: LinkedItemsPromptMode,
    /// Max UTF-8 bytes for the primary ticket description in prompts (`0` = unlimited).
    #[serde(default)]
    pub ticket_context_max_description_bytes: usize,
    /// Max UTF-8 bytes per linked issue description when mode is `full` (`0` = unlimited).
    #[serde(default)]
    pub linked_issue_description_max_bytes: usize,
    /// Pinned Atlassian CLI (`acli`) version to install at runtime; empty = latest.
    /// `acli` is installed on startup (only in Jira mode), not baked into the image.
    #[serde(default)]
    pub acli_version: String,
}

fn default_item_types() -> Vec<String> {
    vec!["Task".to_string(), "Bug".to_string()]
}

fn default_jira_done_status() -> String {
    "Done".to_string()
}

impl Default for JiraConfig {
    fn default() -> Self {
        Self {
            project_keys: Vec::new(),
            item_types: default_item_types(),
            jql_filter: String::new(),
            site: String::new(),
            email: String::new(),
            done_status: default_jira_done_status(),
            linked_items_in_prompt: LinkedItemsPromptMode::default(),
            ticket_context_max_description_bytes: 0,
            linked_issue_description_max_bytes: 0,
            acli_version: String::new(),
        }
    }
}
