use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::error::{MaestroError, Result};
use crate::process;

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

pub struct JiraClient {
    pub repo_path: std::path::PathBuf,
}

impl JiraClient {
    pub fn new(repo_path: std::path::PathBuf) -> Self {
        Self { repo_path }
    }

    pub async fn list_todo_tickets(
        &self,
        project_keys: &[String],
        item_types: &[String],
    ) -> Result<Vec<JiraTicket>> {
        let mut all_tickets = Vec::new();

        for project_key in project_keys {
            for item_type in item_types {
                let output = process::run_shell_command(
                    &format!(
                        "acli jira issue list --project {project_key} --status \"To Do\" --type \"{item_type}\" --output json"
                    ),
                    &self.repo_path,
                    CancellationToken::new(),
                )
                .await?;

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
                all_tickets.extend(tickets);
            }
        }

        // Sort by key to ensure oldest-first deterministic ordering
        all_tickets.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(all_tickets)
    }

    pub async fn get_ticket_details(
        &self,
        key: &str,
        project_keys: &[String],
    ) -> Result<JiraTicket> {
        info!(ticket = key, "Retrieving ticket details");
        let output = process::run_shell_command(
            &format!("acli jira issue view {key} --output json"),
            &self.repo_path,
            CancellationToken::new(),
        )
        .await?;

        if !output.success() {
            return Err(MaestroError::Jira(format!(
                "Failed to get ticket details for {key}: {}",
                output.stderr
            )));
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

    async fn get_linked_item(&self, key: &str, link_type: &str) -> Result<LinkedItem> {
        let output = process::run_shell_command(
            &format!("acli jira issue view {key} --output json"),
            &self.repo_path,
            CancellationToken::new(),
        )
        .await?;

        if !output.success() {
            return Err(MaestroError::Jira(format!(
                "Failed to get linked item {key}: {}",
                output.stderr
            )));
        }

        let mut item = parse_linked_item(&output.stdout)?;
        item.link_type = link_type.to_string();
        Ok(item)
    }
}

fn parse_ticket_list(json_str: &str, default_type: &str) -> Result<Vec<JiraTicket>> {
    let value: serde_json::Value = serde_json::from_str(json_str).map_err(|e| {
        MaestroError::Jira(format!("Failed to parse ticket list JSON: {e}"))
    })?;

    let issues = value
        .get("issues")
        .and_then(|v| v.as_array())
        .or_else(|| value.as_array())
        .unwrap_or(&Vec::new())
        .clone();

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
            description: fields
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
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

fn parse_ticket_detail(json_str: &str) -> Result<JiraTicket> {
    let value: serde_json::Value = serde_json::from_str(json_str).map_err(|e| {
        MaestroError::Jira(format!("Failed to parse ticket detail JSON: {e}"))
    })?;

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
        description: fields
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
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
    let value: serde_json::Value = serde_json::from_str(json_str).map_err(|e| {
        MaestroError::Jira(format!("Failed to parse linked item JSON: {e}"))
    })?;

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
        description: fields
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        status: fields
            .get("status")
            .and_then(|v| v.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        link_type: String::new(),
    })
}
