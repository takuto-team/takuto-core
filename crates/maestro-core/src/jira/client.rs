// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::error::Result;
use crate::jira::JiraError;
use crate::process::{self, CommandOutput};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JiraTicket {
    pub key: String,
    pub summary: String,
    pub description: String,
    pub item_type: String,
    pub status: String,
    pub linked_items: Vec<LinkedItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkedItem {
    pub key: String,
    pub summary: String,
    pub description: String,
    pub status: String,
    pub link_type: String,
}

/// Dashboard manual-start detail modal: Jira description as Markdown (ADF converted when needed).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TicketDescriptionPreview {
    pub key: String,
    pub summary: String,
    pub description_markdown: String,
}

pub struct JiraClient {
    pub repo_path: std::path::PathBuf,
}

impl JiraClient {
    pub fn new(repo_path: std::path::PathBuf) -> Self {
        Self { repo_path }
    }

    async fn acli(&self, args: &[&str]) -> Result<CommandOutput> {
        info!(args = ?args, "Running acli command");
        let output =
            process::run_command("acli", args, &self.repo_path, CancellationToken::new()).await?;
        if !output.success() {
            info!(
                exit_code = output.exit_code,
                stderr = %output.stderr,
                "acli command failed"
            );
        } else {
            debug!(stdout_len = output.stdout.len(), "acli command succeeded");
        }
        Ok(output)
    }

    pub async fn list_todo_tickets(
        &self,
        project_keys: &[String],
        item_types: &[String],
    ) -> Result<Vec<JiraTicket>> {
        let mut all_tickets = Vec::new();

        for project_key in project_keys {
            for item_type in item_types {
                let jql = format!(
                    "project = {project_key} AND status = \"To Do\" AND issuetype = \"{item_type}\""
                );
                info!(
                    project = project_key,
                    item_type = item_type,
                    jql = %jql,
                    "Searching for tickets"
                );

                let search_args = [
                    "jira".to_string(),
                    "workitem".to_string(),
                    "search".to_string(),
                    "--jql".to_string(),
                    jql.clone(),
                    "--json".to_string(),
                    "--limit".to_string(),
                    "50".to_string(),
                ];
                let refs: Vec<&str> = search_args.iter().map(|s| s.as_str()).collect();
                let output = self.acli(&refs).await?;

                if !output.success() {
                    warn!(
                        project = project_key,
                        item_type = item_type,
                        stderr = %output.stderr,
                        "Failed to list tickets, skipping"
                    );
                    continue;
                }

                let tickets = parse_ticket_list(&output.stdout, item_type)?;
                info!(
                    project = project_key,
                    item_type = item_type,
                    count = tickets.len(),
                    "Parsed tickets from response"
                );
                all_tickets.extend(tickets);
            }
        }

        // Sort by key to ensure oldest-first deterministic ordering
        all_tickets.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(all_tickets)
    }

    /// **To Do** issues in the given projects for the dashboard manual-start picker: **excludes Epics**, **`ORDER BY rank ASC`**
    /// (backlog/board order). Ignores **`[jira] item_types`**. When **`jql_filter`** is non-empty, it is **`AND`**-combined so
    /// results can match the same scope as a Jira board filter (paste the board’s JQL fragment there, without duplicating
    /// `project` / `status` if possible).
    pub async fn list_todo_tickets_by_rank(
        &self,
        project_keys: &[String],
        jql_filter: &str,
    ) -> Result<Vec<JiraTicket>> {
        if project_keys.is_empty() {
            return Ok(Vec::new());
        }

        let core = if project_keys.len() == 1 {
            format!(
                r#"project = {} AND status = "To Do" AND issuetype != Epic"#,
                project_keys[0].trim()
            )
        } else {
            let projects = project_keys
                .iter()
                .map(|k| k.trim())
                .filter(|k| !k.is_empty())
                .collect::<Vec<_>>()
                .join(", ");
            format!(r#"project in ({projects}) AND status = "To Do" AND issuetype != Epic"#)
        };

        let extra = jql_filter.trim();
        let jql = if extra.is_empty() {
            format!("{core} ORDER BY rank ASC")
        } else {
            format!("({core}) AND ({extra}) ORDER BY rank ASC")
        };

        info!(jql = %jql, "Searching for To Do tickets (board-style, by rank)");

        let search_args = [
            "jira".to_string(),
            "workitem".to_string(),
            "search".to_string(),
            "--jql".to_string(),
            jql,
            "--json".to_string(),
            "--limit".to_string(),
            "200".to_string(),
        ];
        let refs: Vec<&str> = search_args.iter().map(|s| s.as_str()).collect();
        let output = self.acli(&refs).await?;

        if !output.success() {
            return Err(JiraError::ListTodoFailed {
                stderr: output.stderr,
            }
            .into());
        }

        let mut tickets = parse_ticket_list(&output.stdout, "Issue")?;
        dedupe_tickets_preserve_order(&mut tickets);
        tickets.retain(|t| !t.item_type.eq_ignore_ascii_case("Epic"));
        Ok(tickets)
    }

    /// Fetch **summary** and **description** for the manual-start preview modal (no linked issues).
    pub async fn get_ticket_description_preview(
        &self,
        key: &str,
        project_keys: &[String],
    ) -> Result<TicketDescriptionPreview> {
        let project = key.split('-').next().unwrap_or("").trim();
        if project.is_empty() || !project_keys.iter().any(|p| p.trim() == project) {
            return Err(JiraError::TicketNotInConfiguredProjects {
                key: key.to_string(),
            }
            .into());
        }

        let output = self
            .acli(&[
                "jira",
                "workitem",
                "view",
                key,
                "--json",
                "--fields",
                "key,summary,description",
            ])
            .await?;

        if !output.success() {
            return Err(JiraError::GetDescriptionPreviewFailed {
                key: key.to_string(),
                stderr: output.stderr,
            }
            .into());
        }

        let value: serde_json::Value =
            serde_json::from_str(&output.stdout).map_err(|source| JiraError::ParseTicketJson {
                key: key.to_string(),
                source,
            })?;

        let fields = value.get("fields").unwrap_or(&value);
        let k = value
            .get("key")
            .and_then(|v| v.as_str())
            .unwrap_or(key)
            .to_string();
        let summary = fields
            .get("summary")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let desc_val = fields
            .get("description")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let description_markdown = super::adf_markdown::jira_description_to_markdown(&desc_val);

        Ok(TicketDescriptionPreview {
            key: k,
            summary,
            description_markdown,
        })
    }

    pub async fn get_ticket_details(
        &self,
        key: &str,
        project_keys: &[String],
    ) -> Result<JiraTicket> {
        info!(ticket = key, "Retrieving ticket details");
        let output = self
            .acli(&[
                "jira",
                "workitem",
                "view",
                key,
                "--json",
                "--fields",
                "key,issuetype,summary,status,assignee,description",
            ])
            .await?;

        if !output.success() {
            return Err(JiraError::GetDetailsFailed {
                key: key.to_string(),
                stderr: output.stderr,
            }
            .into());
        }

        let mut ticket = parse_ticket_detail(&output.stdout)?;

        // Fetch linked items (one level deep, only from configured projects)
        let linked_keys = extract_linked_keys(&output.stdout);
        for (linked_key, link_type) in linked_keys {
            let linked_project = linked_key.split('-').next().unwrap_or("");
            if !project_keys.iter().any(|pk| pk == linked_project) {
                debug!(key = %linked_key, "Skipping linked item from non-configured project");
                continue;
            }

            match self.get_linked_item(&linked_key, &link_type).await {
                Ok(item) => ticket.linked_items.push(item),
                Err(e) => {
                    warn!(key = %linked_key, error = %e, "Failed to fetch linked item");
                }
            }
        }

        Ok(ticket)
    }

    pub async fn assign_ticket(&self, key: &str) -> Result<()> {
        info!(ticket = key, "Assigning ticket to self");
        let output = self
            .acli(&[
                "jira",
                "workitem",
                "assign",
                "--key",
                key,
                "--assignee",
                "@me",
                "--yes",
            ])
            .await?;

        if !output.success() {
            return Err(JiraError::AssignFailed {
                key: key.to_string(),
                stderr: output.stderr,
            }
            .into());
        }
        Ok(())
    }

    pub async fn unassign_ticket(&self, key: &str) -> Result<()> {
        info!(ticket = key, "Unassigning ticket");
        let output = self
            .acli(&[
                "jira",
                "workitem",
                "assign",
                "--key",
                key,
                "--remove-assignee",
                "--yes",
            ])
            .await?;

        if !output.success() {
            return Err(JiraError::UnassignFailed {
                key: key.to_string(),
                stderr: output.stderr,
            }
            .into());
        }
        Ok(())
    }

    pub async fn transition_ticket(&self, key: &str, status: &str) -> Result<()> {
        info!(ticket = key, status = status, "Transitioning ticket");
        let output = self
            .acli(&[
                "jira",
                "workitem",
                "transition",
                "--key",
                key,
                "--status",
                status,
                "--yes",
            ])
            .await?;

        if !output.success() {
            return Err(JiraError::TransitionFailed {
                key: key.to_string(),
                status: status.to_string(),
                stderr: output.stderr,
            }
            .into());
        }
        Ok(())
    }

    pub async fn update_description(&self, key: &str, description: &str) -> Result<()> {
        info!(ticket = key, "Updating ticket description");
        let output = self
            .acli(&[
                "jira",
                "workitem",
                "edit",
                "--key",
                key,
                "--description",
                description,
                "--yes",
            ])
            .await?;

        if !output.success() {
            return Err(JiraError::UpdateDescriptionFailed {
                key: key.to_string(),
                stderr: output.stderr,
            }
            .into());
        }
        Ok(())
    }

    async fn get_linked_item(&self, key: &str, link_type: &str) -> Result<LinkedItem> {
        let output = self
            .acli(&[
                "jira",
                "workitem",
                "view",
                key,
                "--json",
                "--fields",
                "key,issuetype,summary,status,description",
            ])
            .await?;

        if !output.success() {
            return Err(JiraError::GetLinkedItemFailed {
                key: key.to_string(),
                stderr: output.stderr,
            }
            .into());
        }

        let mut item = parse_linked_item(&output.stdout)?;
        item.link_type = link_type.to_string();
        Ok(item)
    }
}

fn parse_ticket_list(json_str: &str, default_type: &str) -> Result<Vec<JiraTicket>> {
    // acli returns a JSON array of work items directly
    let issues: Vec<serde_json::Value> =
        serde_json::from_str(json_str).map_err(|source| JiraError::ParseTicketListJson { source })?;

    let mut tickets = Vec::new();
    for issue in &issues {
        let fields = issue.get("fields").unwrap_or(issue);
        let ticket = JiraTicket {
            key: issue
                .get("key")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            summary: fields
                .get("summary")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            description: extract_description_text(fields),
            item_type: fields
                .get("issuetype")
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or(default_type)
                .to_string(),
            status: fields
                .get("status")
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("To Do")
                .to_string(),
            linked_items: Vec::new(),
        };
        if !ticket.key.is_empty() {
            tickets.push(ticket);
        }
    }

    Ok(tickets)
}

fn dedupe_tickets_preserve_order(tickets: &mut Vec<JiraTicket>) {
    let mut seen = std::collections::HashSet::new();
    tickets.retain(|t| seen.insert(t.key.clone()));
}

fn parse_ticket_detail(json_str: &str) -> Result<JiraTicket> {
    let value: serde_json::Value =
        serde_json::from_str(json_str).map_err(|source| JiraError::ParseTicketDetailJson { source })?;

    let fields = value.get("fields").unwrap_or(&value);

    Ok(JiraTicket {
        key: value
            .get("key")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        summary: fields
            .get("summary")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        description: extract_description_text(fields),
        item_type: fields
            .get("issuetype")
            .and_then(|v| v.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("Task")
            .to_string(),
        status: fields
            .get("status")
            .and_then(|v| v.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        linked_items: Vec::new(),
    })
}

/// Extract plain text from Jira ADF (Atlassian Document Format) description.
fn extract_description_text(fields: &serde_json::Value) -> String {
    let Some(desc) = fields.get("description") else {
        return String::new();
    };

    // If it's a simple string, return as-is
    if let Some(s) = desc.as_str() {
        return s.to_string();
    }

    // ADF format: extract text from content nodes recursively
    fn collect_text(node: &serde_json::Value, buf: &mut String) {
        if let Some(text) = node.get("text").and_then(|v| v.as_str()) {
            buf.push_str(text);
        }
        if let Some(content) = node.get("content").and_then(|v| v.as_array()) {
            for child in content {
                collect_text(child, buf);
            }
            // Add newline after paragraph-level nodes
            if let Some(node_type) = node.get("type").and_then(|v| v.as_str())
                && matches!(
                    node_type,
                    "paragraph" | "heading" | "bulletList" | "orderedList"
                )
            {
                buf.push('\n');
            }
        }
    }

    let mut text = String::new();
    collect_text(desc, &mut text);
    text.trim().to_string()
}

/// Returns (key, link_type) pairs extracted from Jira issuelinks.
fn extract_linked_keys(json_str: &str) -> Vec<(String, String)> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json_str) else {
        return Vec::new();
    };

    let fields = value.get("fields").unwrap_or(&value);
    let Some(links) = fields.get("issuelinks").and_then(|v| v.as_array()) else {
        return Vec::new();
    };

    let mut keys = Vec::new();
    for link in links {
        let link_type = link
            .get("type")
            .and_then(|v| v.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("Related")
            .to_string();

        // Outward link (e.g. "blocks OTHER-123")
        if let Some(key) = link
            .get("outwardIssue")
            .and_then(|v| v.get("key"))
            .and_then(|v| v.as_str())
        {
            let outward_label = link
                .get("type")
                .and_then(|v| v.get("outward"))
                .and_then(|v| v.as_str())
                .unwrap_or(&link_type);
            keys.push((key.to_string(), outward_label.to_string()));
        }
        // Inward link (e.g. "is blocked by OTHER-123")
        if let Some(key) = link
            .get("inwardIssue")
            .and_then(|v| v.get("key"))
            .and_then(|v| v.as_str())
        {
            let inward_label = link
                .get("type")
                .and_then(|v| v.get("inward"))
                .and_then(|v| v.as_str())
                .unwrap_or(&link_type);
            keys.push((key.to_string(), inward_label.to_string()));
        }
    }

    keys
}

fn parse_linked_item(json_str: &str) -> Result<LinkedItem> {
    let value: serde_json::Value =
        serde_json::from_str(json_str).map_err(|source| JiraError::ParseLinkedItemJson { source })?;

    let fields = value.get("fields").unwrap_or(&value);

    Ok(LinkedItem {
        key: value
            .get("key")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        summary: fields
            .get("summary")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        description: extract_description_text(fields),
        status: fields
            .get("status")
            .and_then(|v| v.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        link_type: String::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- parse_ticket_list ---

    #[test]
    fn parse_ticket_list_basic() {
        let json = r#"[
            {
                "key": "PROJ-1",
                "fields": {
                    "summary": "Fix login",
                    "description": "Some description",
                    "issuetype": { "name": "Bug" },
                    "status": { "name": "To Do" }
                }
            }
        ]"#;
        let tickets = parse_ticket_list(json, "Task").unwrap();
        assert_eq!(tickets.len(), 1);
        assert_eq!(tickets[0].key, "PROJ-1");
        assert_eq!(tickets[0].summary, "Fix login");
        assert_eq!(tickets[0].item_type, "Bug");
        assert_eq!(tickets[0].status, "To Do");
        assert_eq!(tickets[0].description, "Some description");
    }

    #[test]
    fn parse_ticket_list_uses_default_type_when_missing() {
        let json = r#"[{ "key": "PROJ-2", "fields": { "summary": "Task" } }]"#;
        let tickets = parse_ticket_list(json, "Story").unwrap();
        assert_eq!(tickets[0].item_type, "Story");
    }

    #[test]
    fn parse_ticket_list_skips_empty_keys() {
        let json = r#"[{ "key": "", "fields": { "summary": "No key" } }]"#;
        let tickets = parse_ticket_list(json, "Task").unwrap();
        assert!(tickets.is_empty());
    }

    #[test]
    fn parse_ticket_list_empty_array() {
        let tickets = parse_ticket_list("[]", "Task").unwrap();
        assert!(tickets.is_empty());
    }

    #[test]
    fn parse_ticket_list_invalid_json() {
        assert!(parse_ticket_list("not json", "Task").is_err());
    }

    // --- parse_ticket_detail ---

    #[test]
    fn parse_ticket_detail_full() {
        let json = r#"{
            "key": "PROJ-10",
            "fields": {
                "summary": "Implement feature",
                "description": "Build the feature",
                "issuetype": { "name": "Story" },
                "status": { "name": "In Progress" }
            }
        }"#;
        let ticket = parse_ticket_detail(json).unwrap();
        assert_eq!(ticket.key, "PROJ-10");
        assert_eq!(ticket.summary, "Implement feature");
        assert_eq!(ticket.item_type, "Story");
        assert_eq!(ticket.status, "In Progress");
        assert_eq!(ticket.description, "Build the feature");
        assert!(ticket.linked_items.is_empty());
    }

    #[test]
    fn parse_ticket_detail_missing_fields_uses_defaults() {
        let json = r#"{ "key": "PROJ-99" }"#;
        let ticket = parse_ticket_detail(json).unwrap();
        assert_eq!(ticket.key, "PROJ-99");
        assert_eq!(ticket.summary, "");
        assert_eq!(ticket.item_type, "Task");
        assert_eq!(ticket.status, "");
        assert_eq!(ticket.description, "");
    }

    #[test]
    fn parse_ticket_detail_empty_description() {
        let json = r#"{
            "key": "PROJ-5",
            "fields": { "summary": "No desc", "description": null }
        }"#;
        let ticket = parse_ticket_detail(json).unwrap();
        assert_eq!(ticket.description, "");
    }

    // --- extract_description_text ---

    #[test]
    fn extract_description_text_string() {
        let val = serde_json::json!({ "description": "plain text desc" });
        assert_eq!(extract_description_text(&val), "plain text desc");
    }

    #[test]
    fn extract_description_text_adf() {
        let val = serde_json::json!({
            "description": {
                "type": "doc",
                "content": [
                    {
                        "type": "paragraph",
                        "content": [
                            { "type": "text", "text": "Hello " },
                            { "type": "text", "text": "World" }
                        ]
                    }
                ]
            }
        });
        assert_eq!(extract_description_text(&val), "Hello World");
    }

    #[test]
    fn extract_description_text_missing() {
        let val = serde_json::json!({ "summary": "no description field" });
        assert_eq!(extract_description_text(&val), "");
    }

    #[test]
    fn extract_description_text_null() {
        let val = serde_json::json!({ "description": null });
        assert_eq!(extract_description_text(&val), "");
    }

    // --- extract_linked_keys ---

    #[test]
    fn extract_linked_keys_outward_and_inward() {
        let json = r#"{
            "fields": {
                "issuelinks": [
                    {
                        "type": { "name": "Blocks", "outward": "blocks", "inward": "is blocked by" },
                        "outwardIssue": { "key": "PROJ-20" }
                    },
                    {
                        "type": { "name": "Relates", "outward": "relates to", "inward": "relates to" },
                        "inwardIssue": { "key": "PROJ-30" }
                    }
                ]
            }
        }"#;
        let keys = extract_linked_keys(json);
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0], ("PROJ-20".to_string(), "blocks".to_string()));
        assert_eq!(keys[1], ("PROJ-30".to_string(), "relates to".to_string()));
    }

    #[test]
    fn extract_linked_keys_no_links() {
        let json = r#"{ "fields": {} }"#;
        let keys = extract_linked_keys(json);
        assert!(keys.is_empty());
    }

    #[test]
    fn extract_linked_keys_invalid_json() {
        let keys = extract_linked_keys("not json");
        assert!(keys.is_empty());
    }

    // --- dedupe_tickets_preserve_order ---

    #[test]
    fn dedupe_preserves_first_occurrence() {
        let mut tickets = vec![
            JiraTicket {
                key: "A-1".into(),
                summary: "first".into(),
                description: String::new(),
                item_type: "Task".into(),
                status: "To Do".into(),
                linked_items: vec![],
            },
            JiraTicket {
                key: "B-2".into(),
                summary: "second".into(),
                description: String::new(),
                item_type: "Task".into(),
                status: "To Do".into(),
                linked_items: vec![],
            },
            JiraTicket {
                key: "A-1".into(),
                summary: "duplicate".into(),
                description: String::new(),
                item_type: "Task".into(),
                status: "To Do".into(),
                linked_items: vec![],
            },
        ];
        dedupe_tickets_preserve_order(&mut tickets);
        assert_eq!(tickets.len(), 2);
        assert_eq!(tickets[0].key, "A-1");
        assert_eq!(tickets[0].summary, "first");
        assert_eq!(tickets[1].key, "B-2");
    }

    // --- parse_linked_item ---

    #[test]
    fn parse_linked_item_basic() {
        let json = r#"{
            "key": "OTHER-5",
            "fields": {
                "summary": "Related task",
                "description": "Details here",
                "status": { "name": "Done" }
            }
        }"#;
        let item = parse_linked_item(json).unwrap();
        assert_eq!(item.key, "OTHER-5");
        assert_eq!(item.summary, "Related task");
        assert_eq!(item.description, "Details here");
        assert_eq!(item.status, "Done");
        assert_eq!(item.link_type, ""); // set by caller
    }
}
