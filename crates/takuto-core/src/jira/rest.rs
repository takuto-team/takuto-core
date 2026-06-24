// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Per-user Jira REST client.
//!
//! Unlike [`super::client::JiraClient`], which shells out to the global
//! `acli` binary (single host-wide Atlassian auth), this client uses a
//! **per-user** credential — the Jira site URL plus the `email:token` Basic
//! auth pair pasted by the user — against the Jira Cloud REST API
//! (`/rest/api/3/*`).
//!
//! Two responsibilities:
//!   1. **Validation** ([`validate`]) — `GET /rest/api/3/myself` to confirm a
//!      freshly pasted credential works, capturing the account id / display
//!      name. Mirrors the GitHub PAT validation path (`auth::pat_validation`).
//!   2. **Read path** — [`JiraRestClient`] implements [`super::TicketLister`]
//!      and [`super::TicketReader`] so the poller and the bootstrap
//!      "Retrieve Details" step can read Jira with the owner's own token,
//!      falling back to `acli` when no per-user credential exists.
//!
//! The HTTP boundary is isolated behind the [`JiraHttp`] trait so tests run
//! with a canned mock and never touch the network. The production impl
//! ([`RealJiraHttp`]) shells out to `curl` (the same approach
//! `github_app.rs` already uses for the GitHub API), passing the credential
//! via `--user` so the token is never interpolated into a shell string.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::auth::{SealedBlob, open as seal_open};
use crate::db::Database;
use crate::error::Result;
use crate::process;

use super::client::{JiraTicket, LinkedItem, TicketDescriptionPreview};
use super::source::{TicketLister, TicketReader};

/// A resolved (decrypted) per-user Jira credential.
#[derive(Debug, Clone)]
pub struct JiraRestCredential {
    /// Site base URL, e.g. `https://acme.atlassian.net` (trailing slash trimmed on use).
    pub site: String,
    /// Account email — the Basic-auth username.
    pub email: String,
    /// API token — the Basic-auth password (the sealed secret).
    pub token: String,
    /// Atlassian `accountId` captured at validation — the self-assign target
    /// for `assign_ticket` (`PUT …/assignee {accountId}`).
    pub account_id: String,
}

/// Account identity returned by `GET /rest/api/3/myself`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JiraAccount {
    pub account_id: String,
    pub display_name: String,
    pub email: Option<String>,
}

/// Typed validation outcomes, mirrored on the wire as `{ "error": "<code>" }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JiraValidationError {
    /// 401 — the email/token pair is wrong.
    InvalidToken,
    /// 403 — authenticated but not allowed (e.g. token scope / site policy).
    Unauthorized,
    /// Network / spawn / parse failure — distinct from a bad credential.
    Transport(String),
}

impl JiraValidationError {
    pub fn code(&self) -> &'static str {
        match self {
            JiraValidationError::InvalidToken => "invalid_token",
            JiraValidationError::Unauthorized => "unauthorized",
            JiraValidationError::Transport(_) => "jira_transport_error",
        }
    }
}

/// One Jira REST response: HTTP status + raw body.
#[derive(Debug, Clone)]
pub struct JiraHttpResponse {
    pub status: u16,
    pub body: String,
}

/// HTTP boundary for the Jira REST API. Isolated so tests never hit the
/// network. `Err` is reserved for transport/spawn failures; any HTTP status
/// (including 401/403) is returned as `Ok(JiraHttpResponse{status, ..})`.
#[async_trait]
pub trait JiraHttp: Send + Sync {
    /// `GET {cred.site}/{path}` with `Accept: application/json` and Basic auth
    /// built from `cred.email` + `cred.token`. `path` is relative
    /// (e.g. `rest/api/3/myself`).
    async fn get(
        &self,
        cred: &JiraRestCredential,
        path: &str,
    ) -> std::result::Result<JiraHttpResponse, String>;

    /// `POST {cred.site}/{path}` with a JSON `body` (`Content-Type:
    /// application/json`). Used by transitions.
    async fn post(
        &self,
        cred: &JiraRestCredential,
        path: &str,
        body: &str,
    ) -> std::result::Result<JiraHttpResponse, String>;

    /// `PUT {cred.site}/{path}` with a JSON `body`. Used by assignee /
    /// description writes.
    async fn put(
        &self,
        cred: &JiraRestCredential,
        path: &str,
        body: &str,
    ) -> std::result::Result<JiraHttpResponse, String>;
}

/// Marker appended by `curl -w` so the exit-status line is separable from the
/// JSON body. Chosen to be vanishingly unlikely to appear in a Jira payload.
const STATUS_MARKER: &str = "TAKUTO_HTTP_STATUS:";

/// Production [`JiraHttp`] shelling out to `curl`.
pub struct RealJiraHttp;

impl RealJiraHttp {
    pub fn new() -> Self {
        Self
    }
}

impl Default for RealJiraHttp {
    fn default() -> Self {
        Self::new()
    }
}

impl RealJiraHttp {
    /// Shared curl invocation for all verbs. `body` (when `Some`) is sent as
    /// the request payload with `Content-Type: application/json`. The token is
    /// passed via `--user` (a process arg, never interpolated into a shell
    /// string); the body is passed via `--data` (also a process arg).
    async fn run(
        &self,
        cred: &JiraRestCredential,
        method: &str,
        path: &str,
        body: Option<&str>,
    ) -> std::result::Result<JiraHttpResponse, String> {
        let url = format!(
            "{}/{}",
            cred.site.trim_end_matches('/'),
            path.trim_start_matches('/')
        );
        let userpass = format!("{}:{}", cred.email, cred.token);
        let write_out = format!("\n{STATUS_MARKER}%{{http_code}}");
        let mut args: Vec<String> = vec![
            "-sS".into(),
            "--max-time".into(),
            "30".into(),
            "--connect-timeout".into(),
            "10".into(),
            "-X".into(),
            method.into(),
            "-H".into(),
            "Accept: application/json".into(),
            "--user".into(),
            userpass,
            "-w".into(),
            write_out,
        ];
        if let Some(b) = body {
            args.push("-H".into());
            args.push("Content-Type: application/json".into());
            args.push("--data".into());
            args.push(b.to_string());
        }
        args.push(url);
        let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let cwd = std::env::temp_dir();
        let output = process::run_command("curl", &arg_refs, &cwd, CancellationToken::new())
            .await
            .map_err(|e| format!("spawn curl failed: {e}"))?;
        if !output.success() {
            return Err(format!(
                "curl exit {}: {}",
                output.exit_code,
                output.stderr.trim()
            ));
        }
        parse_status_marker(&output.stdout).ok_or_else(|| {
            format!(
                "could not parse curl response: {} bytes",
                output.stdout.len()
            )
        })
    }
}

#[async_trait]
impl JiraHttp for RealJiraHttp {
    async fn get(
        &self,
        cred: &JiraRestCredential,
        path: &str,
    ) -> std::result::Result<JiraHttpResponse, String> {
        self.run(cred, "GET", path, None).await
    }

    async fn post(
        &self,
        cred: &JiraRestCredential,
        path: &str,
        body: &str,
    ) -> std::result::Result<JiraHttpResponse, String> {
        self.run(cred, "POST", path, Some(body)).await
    }

    async fn put(
        &self,
        cred: &JiraRestCredential,
        path: &str,
        body: &str,
    ) -> std::result::Result<JiraHttpResponse, String> {
        self.run(cred, "PUT", path, Some(body)).await
    }
}

/// Split a `curl -w '\nTAKUTO_HTTP_STATUS:%{http_code}'` stdout into the JSON
/// body and the trailing HTTP status.
fn parse_status_marker(raw: &str) -> Option<JiraHttpResponse> {
    let idx = raw.rfind(STATUS_MARKER)?;
    let status_str = raw[idx + STATUS_MARKER.len()..].trim();
    let status: u16 = status_str.parse().ok()?;
    // Drop the marker (and the newline we prepended to it) from the body.
    let body = raw[..idx].strip_suffix('\n').unwrap_or(&raw[..idx]);
    Some(JiraHttpResponse {
        status,
        body: body.to_string(),
    })
}

/// Map a REST response status to success / typed error.
///
/// - **2xx** → `Ok(resp)` (reads return 200; writes often return 204).
/// - **401 / 403** → [`crate::jira::JiraError::CredentialRejected`] (the stored
///   token was rejected — only reachable on the per-user REST path, where a
///   token is set). The web layer maps this to the `jira_credential_invalid`
///   modal code.
/// - anything else → [`crate::jira::JiraError::ListTodoFailed`] (generic, plain
///   body → `502` at the route).
fn check_rest_status(path: &str, resp: JiraHttpResponse) -> Result<JiraHttpResponse> {
    match resp.status {
        200..=299 => Ok(resp),
        401 | 403 => Err(crate::jira::JiraError::CredentialRejected {
            status: resp.status,
        }
        .into()),
        other => Err(crate::jira::JiraError::ListTodoFailed {
            stderr: format!("Jira REST {path} returned {other}"),
        }
        .into()),
    }
}

/// Validate a credential by calling `GET /rest/api/3/myself`.
pub async fn validate(
    http: &dyn JiraHttp,
    cred: &JiraRestCredential,
) -> std::result::Result<JiraAccount, JiraValidationError> {
    let resp = http
        .get(cred, "rest/api/3/myself")
        .await
        .map_err(JiraValidationError::Transport)?;
    match resp.status {
        200 => {}
        401 => return Err(JiraValidationError::InvalidToken),
        403 => return Err(JiraValidationError::Unauthorized),
        404 => {
            // Wrong site URL (no Jira at this base) reads as a bad credential
            // from the user's point of view.
            return Err(JiraValidationError::InvalidToken);
        }
        other => {
            return Err(JiraValidationError::Transport(format!(
                "GET /rest/api/3/myself returned {other}"
            )));
        }
    }
    let parsed: Value = serde_json::from_str(&resp.body)
        .map_err(|e| JiraValidationError::Transport(format!("myself JSON parse: {e}")))?;
    let account_id = parsed
        .get("accountId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let display_name = parsed
        .get("displayName")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let email = parsed
        .get("emailAddress")
        .and_then(Value::as_str)
        .map(|s| s.to_string());
    if account_id.is_empty() {
        return Err(JiraValidationError::Transport(
            "myself response missing accountId".to_string(),
        ));
    }
    Ok(JiraAccount {
        account_id,
        display_name,
        email,
    })
}

/// Per-user REST-backed Jira read client.
pub struct JiraRestClient {
    http: Arc<dyn JiraHttp>,
    cred: JiraRestCredential,
}

impl JiraRestClient {
    pub fn new(http: Arc<dyn JiraHttp>, cred: JiraRestCredential) -> Self {
        Self { http, cred }
    }

    /// Convenience constructor wiring the production HTTP impl.
    pub fn real(cred: JiraRestCredential) -> Self {
        Self::new(Arc::new(RealJiraHttp::new()), cred)
    }

    async fn get_json(&self, path: &str) -> Result<Value> {
        let resp = self
            .http
            .get(&self.cred, path)
            .await
            .map_err(|stderr| crate::jira::JiraError::ListTodoFailed { stderr })?;
        let resp = check_rest_status(path, resp)?;
        serde_json::from_str(&resp.body)
            .map_err(|source| crate::jira::JiraError::ParseTicketListJson { source }.into())
    }

    /// POST a JSON body, returning the (status-checked) response. Used by the
    /// write actions (transitions). 401/403 → [`JiraError::CredentialRejected`].
    async fn post_checked(&self, path: &str, body: &str) -> Result<JiraHttpResponse> {
        let resp = self
            .http
            .post(&self.cred, path, body)
            .await
            .map_err(|stderr| crate::jira::JiraError::ListTodoFailed { stderr })?;
        check_rest_status(path, resp)
    }

    /// PUT a JSON body, returning the (status-checked) response. Used by the
    /// write actions (assignee, description). 401/403 →
    /// [`JiraError::CredentialRejected`].
    async fn put_checked(&self, path: &str, body: &str) -> Result<JiraHttpResponse> {
        let resp = self
            .http
            .put(&self.cred, path, body)
            .await
            .map_err(|stderr| crate::jira::JiraError::ListTodoFailed { stderr })?;
        check_rest_status(path, resp)
    }

    /// **To Do** issues for the dashboard manual-start picker — REST mirror of
    /// [`super::client::JiraClient::list_todo_tickets_by_rank`]: **excludes
    /// Epics**, all other issue types, **`ORDER BY rank ASC`** (board order),
    /// `AND`-combining a non-empty `jql_filter`. Order from the API is
    /// preserved (no key re-sort).
    pub async fn list_todo_tickets_by_rank(
        &self,
        project_keys: &[String],
        jql_filter: &str,
    ) -> Result<Vec<JiraTicket>> {
        if project_keys.is_empty() {
            return Ok(Vec::new());
        }
        let projects = project_keys
            .iter()
            .map(|k| k.trim())
            .filter(|k| !k.is_empty())
            .map(|k| format!("\"{}\"", k.replace('"', "")))
            .collect::<Vec<_>>()
            .join(", ");
        let core = format!("project in ({projects}) AND status = \"To Do\" AND issuetype != Epic");
        let extra = jql_filter.trim();
        let jql = if extra.is_empty() {
            format!("{core} ORDER BY rank ASC")
        } else {
            format!("({core}) AND ({extra}) ORDER BY rank ASC")
        };
        let encoded = url_encode(&jql);
        // Enhanced search endpoint (`/search/jql`): Atlassian removed the
        // classic `GET /rest/api/3/search` (now returns 410 Gone). Same query
        // params; response still has `issues: [...]` (plus nextPageToken/isLast
        // we ignore — the first page of 200 is enough for the picker).
        let path = format!(
            "rest/api/3/search/jql?jql={encoded}&maxResults=200&fields=summary,issuetype,status,description"
        );
        let value = self.get_json(&path).await?;
        let issues = value
            .get("issues")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        // Preserve the API's rank ordering; only drop empty-key and any Epic
        // that slipped through (defence-in-depth — the JQL already excludes it).
        let tickets: Vec<JiraTicket> = issues
            .iter()
            .map(|i| issue_to_ticket(i, "Task"))
            .filter(|t| !t.key.is_empty() && !t.item_type.eq_ignore_ascii_case("Epic"))
            .collect();
        Ok(tickets)
    }

    /// **summary** + **description** for the manual-start preview modal — REST
    /// mirror of [`super::client::JiraClient::get_ticket_description_preview`].
    /// Rejects a key whose project prefix is not in `project_keys` with
    /// [`crate::jira::JiraError::TicketNotInConfiguredProjects`] (the route maps
    /// it to `403`).
    pub async fn get_ticket_description_preview(
        &self,
        key: &str,
        project_keys: &[String],
    ) -> Result<TicketDescriptionPreview> {
        let project = key.split('-').next().unwrap_or("").trim();
        if project.is_empty() || !project_keys.iter().any(|p| p.trim() == project) {
            return Err(crate::jira::JiraError::TicketNotInConfiguredProjects {
                key: key.to_string(),
            }
            .into());
        }
        let path = format!("rest/api/3/issue/{key}?fields=summary,description");
        let value = self.get_json(&path).await?;
        let fields = value.get("fields").unwrap_or(&value);
        let k = value
            .get("key")
            .and_then(Value::as_str)
            .unwrap_or(key)
            .to_string();
        let summary = fields
            .get("summary")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let desc_val = fields.get("description").cloned().unwrap_or(Value::Null);
        let description_markdown = super::adf_markdown::jira_description_to_markdown(&desc_val);
        Ok(TicketDescriptionPreview {
            key: k,
            summary,
            description_markdown,
        })
    }

    // ── write actions (REST mirrors of the acli ExternalActions methods) ──────

    /// Self-assign `key` to the credential's account
    /// (`PUT /rest/api/3/issue/{key}/assignee {accountId}`).
    pub async fn assign_ticket(&self, key: &str) -> Result<()> {
        let body = serde_json::json!({ "accountId": self.cred.account_id }).to_string();
        self.put_checked(&format!("rest/api/3/issue/{key}/assignee"), &body)
            .await?;
        Ok(())
    }

    /// Clear the assignee (`PUT …/assignee {accountId: null}`).
    pub async fn unassign_ticket(&self, key: &str) -> Result<()> {
        let body = serde_json::json!({ "accountId": Value::Null }).to_string();
        self.put_checked(&format!("rest/api/3/issue/{key}/assignee"), &body)
            .await?;
        Ok(())
    }

    /// Transition `key` to the named target **status** (e.g. "In Progress",
    /// "To Do", the configured done status). acli takes a status name; REST
    /// takes a transition id, so this `GET …/transitions`, resolves the target
    /// status name case-insensitively (preferring the transition's resulting
    /// `to.name`, then its own `name`), then `POST …/transitions {transition:{id}}`.
    /// No matching transition → a typed non-auth [`crate::jira::JiraError::TransitionFailed`].
    pub async fn transition_ticket(&self, key: &str, status: &str) -> Result<()> {
        let target = status.trim();
        let list = self
            .get_json(&format!("rest/api/3/issue/{key}/transitions"))
            .await?;
        let transitions = list
            .get("transitions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let tid_of = |t: &Value| t.get("id").and_then(Value::as_str).map(str::to_string);
        // Pass 1: match the resulting status (`to.name`). Pass 2: the transition's own name.
        let id = transitions
            .iter()
            .find(|t| {
                t.get("to")
                    .and_then(|v| v.get("name"))
                    .and_then(Value::as_str)
                    .is_some_and(|n| n.eq_ignore_ascii_case(target))
            })
            .and_then(tid_of)
            .or_else(|| {
                transitions
                    .iter()
                    .find(|t| {
                        t.get("name")
                            .and_then(Value::as_str)
                            .is_some_and(|n| n.eq_ignore_ascii_case(target))
                    })
                    .and_then(tid_of)
            });
        // Pass 3: language-independent fallback. Jira localizes the default
        // status NAMES in the API to the account's language (e.g. the English
        // "Done" status returns as "Terminé(e)" for a French account), so the
        // name passes miss even when the user configured the status they see in
        // the Jira UI. Map the target intent to a `statusCategory` key
        // ("new"/"indeterminate"/"done") — which Jira never localizes — and take
        // the first transition whose target status is in that category.
        let target_category: Option<&str> = {
            let t = target.to_ascii_lowercase();
            if t.contains("done")
                || t.contains("complete")
                || t.contains("resolved")
                || t.contains("closed")
                || t.contains("termin")
            {
                Some("done")
            } else if t.contains("progress") || t.contains("doing") || t.contains("cours") {
                Some("indeterminate")
            } else if t == "to do" || t == "todo" || t.contains("backlog") || t.contains("faire") {
                Some("new")
            } else {
                None
            }
        };
        let id = id.or_else(|| {
            let cat = target_category?;
            transitions
                .iter()
                .find(|t| {
                    t.get("to")
                        .and_then(|v| v.get("statusCategory"))
                        .and_then(|c| c.get("key"))
                        .and_then(Value::as_str)
                        .is_some_and(|k| k.eq_ignore_ascii_case(cat))
                })
                .and_then(tid_of)
        });
        let Some(id) = id else {
            let available = transitions
                .iter()
                .map(|t| {
                    let id = t.get("id").and_then(Value::as_str).unwrap_or("?");
                    let name = t.get("name").and_then(Value::as_str).unwrap_or("?");
                    let to = t
                        .get("to")
                        .and_then(|v| v.get("name"))
                        .and_then(Value::as_str)
                        .unwrap_or("?");
                    format!("{id}:{name}->{to}")
                })
                .collect::<Vec<_>>()
                .join(", ");
            warn!(
                ticket = %key,
                target = %target,
                count = transitions.len(),
                available = %available,
                "No Jira transition matched target status (by to.name or transition name)"
            );
            // User-facing target status names (the values to put in the Done-status
            // setting). Jira localizes status names (e.g. "Terminé(e)"), so a literal
            // English "Done" won't match — surface the actual options so the user can
            // fix the setting.
            let available_statuses = transitions
                .iter()
                .filter_map(|t| {
                    t.get("to")
                        .and_then(|v| v.get("name"))
                        .and_then(Value::as_str)
                })
                .collect::<Vec<_>>()
                .join(", ");
            let stderr = if available_statuses.is_empty() {
                "Jira returned no available transitions for this issue.".to_string()
            } else {
                format!(
                    "no transition leads to status \"{target}\". Available statuses: {available_statuses}"
                )
            };
            return Err(crate::jira::JiraError::TransitionFailed {
                key: key.to_string(),
                status: target.to_string(),
                stderr,
            }
            .into());
        };
        let body = serde_json::json!({ "transition": { "id": id } }).to_string();
        self.post_checked(&format!("rest/api/3/issue/{key}/transitions"), &body)
            .await?;
        Ok(())
    }

    /// Set the issue description (`PUT /rest/api/3/issue/{key} {fields:{description:<ADF>}}`).
    /// The editor sends Markdown/plain text; v1 wraps it as a minimal ADF
    /// document (paragraphs split on blank lines) so the text round-trips —
    /// rich Markdown formatting degrades to plain text (see [`markdown_to_adf`]).
    pub async fn update_description(&self, key: &str, description: &str) -> Result<()> {
        let adf = markdown_to_adf(description);
        let body = serde_json::json!({ "fields": { "description": adf } }).to_string();
        self.put_checked(&format!("rest/api/3/issue/{key}"), &body)
            .await?;
        Ok(())
    }
}

/// Minimal Markdown/plain-text → ADF document. Splits on blank lines into
/// paragraphs, each a single text node. This is the documented v1 fallback
/// (no Markdown→ADF rich conversion yet) — the saved text appears on the issue,
/// but inline/block formatting is flattened to plain text.
fn markdown_to_adf(text: &str) -> Value {
    let paragraphs: Vec<Value> = text
        .split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| {
            serde_json::json!({
                "type": "paragraph",
                "content": [ { "type": "text", "text": p } ]
            })
        })
        .collect();
    let content = if paragraphs.is_empty() {
        // An empty description still needs a valid (empty) paragraph node.
        vec![serde_json::json!({ "type": "paragraph", "content": [] })]
    } else {
        paragraphs
    };
    serde_json::json!({ "type": "doc", "version": 1, "content": content })
}

/// Build a [`JiraTicket`] from one `/rest/api/3` issue object.
fn issue_to_ticket(issue: &Value, default_type: &str) -> JiraTicket {
    let fields = issue.get("fields").unwrap_or(issue);
    JiraTicket {
        key: issue
            .get("key")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        summary: fields
            .get("summary")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        description: fields
            .get("description")
            .map(super::adf_markdown::jira_description_to_markdown)
            .unwrap_or_default(),
        item_type: fields
            .get("issuetype")
            .and_then(|v| v.get("name"))
            .and_then(Value::as_str)
            .unwrap_or(default_type)
            .to_string(),
        status: fields
            .get("status")
            .and_then(|v| v.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        linked_items: Vec::new(),
    }
}

#[async_trait]
impl TicketLister for JiraRestClient {
    async fn list_todo_tickets(
        &self,
        project_keys: &[String],
        item_types: &[String],
    ) -> Result<Vec<JiraTicket>> {
        if project_keys.is_empty() {
            return Ok(Vec::new());
        }
        let projects = project_keys
            .iter()
            .map(|k| format!("\"{}\"", k.trim().replace('"', "")))
            .collect::<Vec<_>>()
            .join(", ");
        let mut jql = format!("project in ({projects}) AND status = \"To Do\"");
        if !item_types.is_empty() {
            let types = item_types
                .iter()
                .map(|t| format!("\"{}\"", t.trim().replace('"', "")))
                .collect::<Vec<_>>()
                .join(", ");
            jql.push_str(&format!(" AND issuetype in ({types})"));
        }
        jql.push_str(" ORDER BY key ASC");
        let encoded = url_encode(&jql);
        // Enhanced search endpoint — classic `GET /rest/api/3/search` was
        // removed by Atlassian (410 Gone). See `list_todo_tickets_by_rank`.
        let path = format!(
            "rest/api/3/search/jql?jql={encoded}&maxResults=50&fields=summary,issuetype,status,description"
        );
        let value = self.get_json(&path).await?;
        let issues = value
            .get("issues")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut tickets: Vec<JiraTicket> = issues
            .iter()
            .map(|i| issue_to_ticket(i, "Task"))
            .filter(|t| !t.key.is_empty())
            .collect();
        tickets.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(tickets)
    }
}

#[async_trait]
impl TicketReader for JiraRestClient {
    async fn get_ticket_details(&self, key: &str, project_keys: &[String]) -> Result<JiraTicket> {
        let path = format!(
            "rest/api/3/issue/{key}?fields=summary,issuetype,status,description,issuelinks"
        );
        let value = self.get_json(&path).await?;
        let mut ticket = issue_to_ticket(&value, "Task");
        if ticket.key.is_empty() {
            ticket.key = key.to_string();
        }

        // Resolve linked items one level deep, restricted to configured projects.
        for (linked_key, link_type) in extract_linked_keys(&value) {
            let linked_project = linked_key.split('-').next().unwrap_or("");
            if !project_keys.iter().any(|pk| pk == linked_project) {
                continue;
            }
            let linked_path = format!(
                "rest/api/3/issue/{linked_key}?fields=summary,issuetype,status,description"
            );
            match self.get_json(&linked_path).await {
                Ok(linked_value) => {
                    let lt = issue_to_ticket(&linked_value, "Task");
                    ticket.linked_items.push(LinkedItem {
                        key: lt.key,
                        summary: lt.summary,
                        description: lt.description,
                        status: lt.status,
                        link_type,
                    });
                }
                Err(e) => {
                    warn!(key = %linked_key, error = %e, "Failed to fetch linked Jira item via REST");
                }
            }
        }
        Ok(ticket)
    }
}

/// Extract `(key, link_label)` pairs from an issue's `issuelinks` field.
fn extract_linked_keys(value: &Value) -> Vec<(String, String)> {
    let fields = value.get("fields").unwrap_or(value);
    let Some(links) = fields.get("issuelinks").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut keys = Vec::new();
    for link in links {
        let type_name = link
            .get("type")
            .and_then(|v| v.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("Related");
        if let Some(key) = link
            .get("outwardIssue")
            .and_then(|v| v.get("key"))
            .and_then(Value::as_str)
        {
            let label = link
                .get("type")
                .and_then(|v| v.get("outward"))
                .and_then(Value::as_str)
                .unwrap_or(type_name);
            keys.push((key.to_string(), label.to_string()));
        }
        if let Some(key) = link
            .get("inwardIssue")
            .and_then(|v| v.get("key"))
            .and_then(Value::as_str)
        {
            let label = link
                .get("type")
                .and_then(|v| v.get("inward"))
                .and_then(Value::as_str)
                .unwrap_or(type_name);
            keys.push((key.to_string(), label.to_string()));
        }
    }
    keys
}

/// Minimal percent-encoding for a JQL query string going into a URL query
/// parameter. Encodes everything that is not an unreserved character.
fn url_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len() * 3);
    for b in input.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Resolve a user's per-user Jira credential from the DB and decrypt the
/// token with the master key. Returns `None` when there is no row, no master
/// key, or the token cannot be decrypted (logged, treated as "no credential"
/// so callers fall back to `acli`).
pub async fn resolve_rest_credential(db: &Database, user_id: &str) -> Option<JiraRestCredential> {
    let row = match crate::db::jira_credentials::find(db.adapter(), user_id).await {
        Ok(Some(row)) => row,
        Ok(None) => return None,
        Err(e) => {
            warn!(error = %e, "Failed to read user Jira credential");
            return None;
        }
    };
    let mk = db.master_key()?;
    let sealed = SealedBlob {
        ciphertext: row.ciphertext,
        nonce: row.nonce,
        wrapped_dek: row.wrapped_dek,
        wnonce: row.wnonce,
    };
    let token_bytes = match seal_open(&mk.key, &sealed) {
        Ok(b) => b,
        Err(e) => {
            warn!(error = %e, "Failed to decrypt user Jira token");
            return None;
        }
    };
    let token = String::from_utf8(token_bytes).ok()?;
    Some(JiraRestCredential {
        site: row.site,
        email: row.email,
        token,
        account_id: row.account_id,
    })
}

/// [`TicketListerFactory`] that prefers a per-user REST credential and falls
/// back to the global `acli` [`JiraClient`] when none is configured.
///
/// `db` + `owner_id` are the poller's resolved owner; when either is absent,
/// or the owner has no credential, the factory yields the `acli` lister.
pub struct DbBackedJiraSourceFactory {
    db: Option<Database>,
    owner_id: Option<String>,
}

impl DbBackedJiraSourceFactory {
    pub fn new(db: Option<Database>, owner_id: Option<String>) -> Self {
        Self { db, owner_id }
    }
}

impl super::source::TicketListerFactory for DbBackedJiraSourceFactory {
    fn lister(&self, repo_path: PathBuf) -> Arc<dyn TicketLister> {
        Arc::new(ResolvingJiraLister {
            repo_path,
            db: self.db.clone(),
            owner_id: self.owner_id.clone(),
        })
    }
}

/// Lister that resolves the per-user credential lazily at list time (async),
/// using REST when present and `acli` otherwise.
struct ResolvingJiraLister {
    repo_path: PathBuf,
    db: Option<Database>,
    owner_id: Option<String>,
}

#[async_trait]
impl TicketLister for ResolvingJiraLister {
    async fn list_todo_tickets(
        &self,
        project_keys: &[String],
        item_types: &[String],
    ) -> Result<Vec<JiraTicket>> {
        if let (Some(db), Some(owner)) = (self.db.as_ref(), self.owner_id.as_ref())
            && let Some(cred) = resolve_rest_credential(db, owner).await
        {
            return JiraRestClient::real(cred)
                .list_todo_tickets(project_keys, item_types)
                .await;
        }
        super::client::JiraClient::new(self.repo_path.clone())
            .list_todo_tickets(project_keys, item_types)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    /// One recorded request: (method, path, body).
    type Call = (String, String, Option<String>);

    struct MockHttp {
        // Keyed by "METHOD path" so GET/POST/PUT to the same path are distinct.
        responses: Mutex<std::collections::HashMap<String, JiraHttpResponse>>,
        calls: Mutex<Vec<Call>>,
    }

    impl MockHttp {
        fn new() -> Self {
            Self {
                responses: Mutex::new(std::collections::HashMap::new()),
                calls: Mutex::new(Vec::new()),
            }
        }
        /// Canned GET response (kept for the existing read tests).
        fn with(self, path: &str, status: u16, body: &str) -> Self {
            self.with_method("GET", path, status, body)
        }
        /// Canned response for an explicit method (POST/PUT/GET).
        fn with_method(self, method: &str, path: &str, status: u16, body: &str) -> Self {
            self.responses.lock().unwrap().insert(
                format!("{method} {path}"),
                JiraHttpResponse {
                    status,
                    body: body.to_string(),
                },
            );
            self
        }
        fn calls(&self) -> Vec<Call> {
            self.calls.lock().unwrap().clone()
        }
        fn lookup(
            &self,
            method: &str,
            path: &str,
            body: Option<&str>,
        ) -> std::result::Result<JiraHttpResponse, String> {
            self.calls.lock().unwrap().push((
                method.to_string(),
                path.to_string(),
                body.map(str::to_string),
            ));
            self.responses
                .lock()
                .unwrap()
                .get(&format!("{method} {path}"))
                .cloned()
                .ok_or_else(|| format!("no canned response for {method} {path}"))
        }
    }

    #[async_trait]
    impl JiraHttp for MockHttp {
        async fn get(
            &self,
            _cred: &JiraRestCredential,
            path: &str,
        ) -> std::result::Result<JiraHttpResponse, String> {
            self.lookup("GET", path, None)
        }
        async fn post(
            &self,
            _cred: &JiraRestCredential,
            path: &str,
            body: &str,
        ) -> std::result::Result<JiraHttpResponse, String> {
            self.lookup("POST", path, Some(body))
        }
        async fn put(
            &self,
            _cred: &JiraRestCredential,
            path: &str,
            body: &str,
        ) -> std::result::Result<JiraHttpResponse, String> {
            self.lookup("PUT", path, Some(body))
        }
    }

    fn cred() -> JiraRestCredential {
        JiraRestCredential {
            site: "https://acme.atlassian.net".into(),
            email: "a@acme.com".into(),
            token: "tok".into(),
            account_id: "acct-123".into(),
        }
    }

    #[test]
    fn parse_status_marker_splits_body_and_status() {
        let raw = "{\"accountId\":\"x\"}\nTAKUTO_HTTP_STATUS:200";
        let resp = parse_status_marker(raw).unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, "{\"accountId\":\"x\"}");
    }

    #[test]
    fn parse_status_marker_handles_empty_body() {
        let raw = "\nTAKUTO_HTTP_STATUS:401";
        let resp = parse_status_marker(raw).unwrap();
        assert_eq!(resp.status, 401);
        assert_eq!(resp.body, "");
    }

    #[tokio::test]
    async fn validate_ok_parses_account() {
        let http = MockHttp::new().with(
            "rest/api/3/myself",
            200,
            "{\"accountId\":\"abc\",\"displayName\":\"Alice\",\"emailAddress\":\"a@acme.com\"}",
        );
        let acct = validate(&http, &cred()).await.unwrap();
        assert_eq!(acct.account_id, "abc");
        assert_eq!(acct.display_name, "Alice");
        assert_eq!(acct.email.as_deref(), Some("a@acme.com"));
    }

    #[tokio::test]
    async fn validate_401_is_invalid_token() {
        let http = MockHttp::new().with("rest/api/3/myself", 401, "{}");
        let err = validate(&http, &cred()).await.unwrap_err();
        assert_eq!(err, JiraValidationError::InvalidToken);
        assert_eq!(err.code(), "invalid_token");
    }

    #[tokio::test]
    async fn validate_403_is_unauthorized() {
        let http = MockHttp::new().with("rest/api/3/myself", 403, "{}");
        let err = validate(&http, &cred()).await.unwrap_err();
        assert_eq!(err, JiraValidationError::Unauthorized);
    }

    #[tokio::test]
    async fn validate_transport_error_is_distinct() {
        let http = MockHttp::new(); // no canned response → Err
        let err = validate(&http, &cred()).await.unwrap_err();
        assert!(matches!(err, JiraValidationError::Transport(_)));
        assert_eq!(err.code(), "jira_transport_error");
    }

    #[tokio::test]
    async fn list_todo_parses_search_issues() {
        let jql = url_encode(
            "project in (\"PROJ\") AND status = \"To Do\" AND issuetype in (\"Task\") ORDER BY key ASC",
        );
        let path = format!(
            "rest/api/3/search/jql?jql={jql}&maxResults=50&fields=summary,issuetype,status,description"
        );
        let body = r#"{"issues":[
            {"key":"PROJ-2","fields":{"summary":"Two","issuetype":{"name":"Task"},"status":{"name":"To Do"}}},
            {"key":"PROJ-1","fields":{"summary":"One","issuetype":{"name":"Bug"},"status":{"name":"To Do"}}}
        ]}"#;
        let http = MockHttp::new().with(&path, 200, body);
        let client = JiraRestClient::new(Arc::new(http), cred());
        let tickets = client
            .list_todo_tickets(&["PROJ".to_string()], &["Task".to_string()])
            .await
            .unwrap();
        assert_eq!(tickets.len(), 2);
        // Sorted by key ascending.
        assert_eq!(tickets[0].key, "PROJ-1");
        assert_eq!(tickets[1].key, "PROJ-2");
    }

    // ── list_todo_tickets_by_rank (manual picker REST path) ───────────────

    #[tokio::test]
    async fn list_by_rank_builds_jql_excludes_epic_and_preserves_order() {
        // No jql_filter: core JQL with rank order, multi-project `in (...)`.
        let jql = url_encode(
            "project in (\"PROJ\", \"OPS\") AND status = \"To Do\" AND issuetype != Epic ORDER BY rank ASC",
        );
        let path = format!(
            "rest/api/3/search/jql?jql={jql}&maxResults=200&fields=summary,issuetype,status,description"
        );
        // API returns PROJ-9 before PROJ-1 (board/rank order) — must be preserved
        // (NOT re-sorted by key). An Epic is dropped defensively.
        let body = r#"{"issues":[
            {"key":"PROJ-9","fields":{"summary":"Nine","issuetype":{"name":"Task"},"status":{"name":"To Do"}}},
            {"key":"PROJ-3","fields":{"summary":"Epic","issuetype":{"name":"Epic"},"status":{"name":"To Do"}}},
            {"key":"PROJ-1","fields":{"summary":"One","issuetype":{"name":"Bug"},"status":{"name":"To Do"}}}
        ]}"#;
        let http = MockHttp::new().with(&path, 200, body);
        let client = JiraRestClient::new(Arc::new(http), cred());
        let tickets = client
            .list_todo_tickets_by_rank(&["PROJ".to_string(), "OPS".to_string()], "")
            .await
            .unwrap();
        let keys: Vec<&str> = tickets.iter().map(|t| t.key.as_str()).collect();
        assert_eq!(
            keys,
            vec!["PROJ-9", "PROJ-1"],
            "rank order kept, Epic dropped"
        );
    }

    #[tokio::test]
    async fn list_by_rank_and_combines_jql_filter() {
        let jql = url_encode(
            "(project in (\"PROJ\") AND status = \"To Do\" AND issuetype != Epic) AND (labels = urgent) ORDER BY rank ASC",
        );
        let path = format!(
            "rest/api/3/search/jql?jql={jql}&maxResults=200&fields=summary,issuetype,status,description"
        );
        let http = MockHttp::new().with(&path, 200, r#"{"issues":[]}"#);
        let client = JiraRestClient::new(Arc::new(http), cred());
        // Succeeds only if the exact AND-combined JQL path was requested.
        let tickets = client
            .list_todo_tickets_by_rank(&["PROJ".to_string()], "labels = urgent")
            .await
            .unwrap();
        assert!(tickets.is_empty());
    }

    #[tokio::test]
    async fn list_by_rank_empty_keys_returns_empty_without_http() {
        let client = JiraRestClient::new(Arc::new(MockHttp::new()), cred());
        assert!(
            client
                .list_todo_tickets_by_rank(&[], "")
                .await
                .unwrap()
                .is_empty()
        );
    }

    // ── get_ticket_description_preview (manual picker REST path) ───────────

    #[tokio::test]
    async fn preview_returns_summary_and_markdown() {
        let path = "rest/api/3/issue/PROJ-5?fields=summary,description";
        let body = r#"{"key":"PROJ-5","fields":{"summary":"A title","description":"plain body"}}"#;
        let http = MockHttp::new().with(path, 200, body);
        let client = JiraRestClient::new(Arc::new(http), cred());
        let preview = client
            .get_ticket_description_preview("PROJ-5", &["PROJ".to_string()])
            .await
            .unwrap();
        assert_eq!(preview.key, "PROJ-5");
        assert_eq!(preview.summary, "A title");
        assert_eq!(preview.description_markdown, "plain body");
    }

    #[tokio::test]
    async fn preview_rejects_out_of_project_key_without_http() {
        // No canned response: if it tried an HTTP call it would error with a
        // different message. It must reject BEFORE any call, with the
        // "not in configured" error the route maps to 403.
        let client = JiraRestClient::new(Arc::new(MockHttp::new()), cred());
        let err = client
            .get_ticket_description_preview("OTHER-1", &["PROJ".to_string()])
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("not in configured"),
            "expected not-in-configured error, got: {err}"
        );
    }

    #[tokio::test]
    async fn get_ticket_details_parses_issue() {
        let path =
            "rest/api/3/issue/PROJ-10?fields=summary,issuetype,status,description,issuelinks";
        let body = r#"{"key":"PROJ-10","fields":{
            "summary":"Implement",
            "issuetype":{"name":"Story"},
            "status":{"name":"In Progress"},
            "description":"plain text"
        }}"#;
        let http = MockHttp::new().with(path, 200, body);
        let client = JiraRestClient::new(Arc::new(http), cred());
        let ticket = client
            .get_ticket_details("PROJ-10", &["PROJ".to_string()])
            .await
            .unwrap();
        assert_eq!(ticket.key, "PROJ-10");
        assert_eq!(ticket.summary, "Implement");
        assert_eq!(ticket.item_type, "Story");
        assert_eq!(ticket.status, "In Progress");
    }

    // ── issue_to_ticket fallbacks ─────────────────────────────────────────

    #[test]
    fn issue_to_ticket_falls_back_for_missing_fields() {
        // No "fields" object → the issue itself is used; missing keys default,
        // and issuetype falls back to the supplied default_type.
        let bare = json!({ "key": "K-1" });
        let t = issue_to_ticket(&bare, "DefaultType");
        assert_eq!(t.key, "K-1");
        assert_eq!(t.summary, "");
        assert_eq!(t.status, "");
        assert_eq!(t.item_type, "DefaultType");
        assert!(t.linked_items.is_empty());
    }

    // ── extract_linked_keys ───────────────────────────────────────────────

    #[test]
    fn extract_linked_keys_reads_outward_and_inward() {
        let v = json!({ "fields": { "issuelinks": [
            { "type": { "name": "Blocks", "outward": "blocks" }, "outwardIssue": { "key": "A-1" } },
            { "type": { "name": "Relates" }, "inwardIssue": { "key": "B-2" } }
        ]}});
        let keys = extract_linked_keys(&v);
        // Outward uses the "outward" label; inward with no "inward" label falls
        // back to the type name.
        assert!(keys.contains(&("A-1".to_string(), "blocks".to_string())));
        assert!(keys.contains(&("B-2".to_string(), "Relates".to_string())));
    }

    #[test]
    fn extract_linked_keys_empty_without_issuelinks() {
        assert!(extract_linked_keys(&json!({ "fields": {} })).is_empty());
    }

    // ── url_encode ────────────────────────────────────────────────────────

    #[test]
    fn url_encode_percent_encodes_reserved_only() {
        assert_eq!(url_encode("a b"), "a%20b");
        assert_eq!(url_encode("\"x\""), "%22x%22");
        // Unreserved characters pass through untouched.
        assert_eq!(url_encode("aZ09-_.~"), "aZ09-_.~");
    }

    // ── list / detail edge + error paths ──────────────────────────────────

    #[tokio::test]
    async fn list_todo_empty_project_keys_returns_empty() {
        let client = JiraRestClient::new(Arc::new(MockHttp::new()), cred());
        let out = client
            .list_todo_tickets(&[], &["Task".to_string()])
            .await
            .unwrap();
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn get_json_non_200_is_error() {
        let jql = url_encode(
            "project in (\"P\") AND status = \"To Do\" AND issuetype in (\"Task\") ORDER BY key ASC",
        );
        let path = format!(
            "rest/api/3/search/jql?jql={jql}&maxResults=50&fields=summary,issuetype,status,description"
        );
        let http = MockHttp::new().with(&path, 500, "boom");
        let client = JiraRestClient::new(Arc::new(http), cred());
        assert!(
            client
                .list_todo_tickets(&["P".to_string()], &["Task".to_string()])
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn get_ticket_details_resolves_in_project_links_and_skips_others() {
        let main_path =
            "rest/api/3/issue/PROJ-10?fields=summary,issuetype,status,description,issuelinks";
        let main_body = r#"{"key":"PROJ-10","fields":{
            "summary":"Main","issuetype":{"name":"Task"},"status":{"name":"To Do"},
            "issuelinks":[
                {"type":{"name":"Blocks","outward":"blocks"},"outwardIssue":{"key":"PROJ-11"}},
                {"type":{"name":"Blocks","outward":"blocks"},"outwardIssue":{"key":"OTHER-1"}},
                {"type":{"name":"Blocks","outward":"blocks"},"outwardIssue":{"key":"PROJ-12"}}
            ]}}"#;
        let linked_path = "rest/api/3/issue/PROJ-11?fields=summary,issuetype,status,description";
        let linked_body = r#"{"key":"PROJ-11","fields":{"summary":"Linked","issuetype":{"name":"Bug"},"status":{"name":"Done"}}}"#;
        // PROJ-11 has a canned response; PROJ-12 does NOT (→ fetch error, warned
        // and skipped); OTHER-1 is out of project (→ filtered before fetch).
        let http =
            MockHttp::new()
                .with(main_path, 200, main_body)
                .with(linked_path, 200, linked_body);
        let client = JiraRestClient::new(Arc::new(http), cred());
        let ticket = client
            .get_ticket_details("PROJ-10", &["PROJ".to_string()])
            .await
            .unwrap();
        assert_eq!(ticket.linked_items.len(), 1);
        assert_eq!(ticket.linked_items[0].key, "PROJ-11");
        assert_eq!(ticket.linked_items[0].link_type, "blocks");
    }

    // ── write actions (REST) ──────────────────────────────────────────────

    #[tokio::test]
    async fn assign_ticket_puts_assignee_with_account_id() {
        let http = MockHttp::new().with_method("PUT", "rest/api/3/issue/PROJ-1/assignee", 204, "");
        let mock = Arc::new(http);
        let client = JiraRestClient::new(mock.clone(), cred());
        client.assign_ticket("PROJ-1").await.unwrap();
        let calls = mock.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "PUT");
        assert_eq!(calls[0].1, "rest/api/3/issue/PROJ-1/assignee");
        let body: Value = serde_json::from_str(calls[0].2.as_deref().unwrap()).unwrap();
        assert_eq!(body["accountId"], "acct-123");
    }

    #[tokio::test]
    async fn unassign_ticket_puts_null_account_id() {
        let http = MockHttp::new().with_method("PUT", "rest/api/3/issue/PROJ-1/assignee", 204, "");
        let mock = Arc::new(http);
        let client = JiraRestClient::new(mock.clone(), cred());
        client.unassign_ticket("PROJ-1").await.unwrap();
        let body: Value = serde_json::from_str(mock.calls()[0].2.as_deref().unwrap()).unwrap();
        assert!(body["accountId"].is_null());
    }

    #[tokio::test]
    async fn transition_matches_by_to_name_then_posts_id() {
        let list = r#"{"transitions":[
            {"id":"11","name":"Start","to":{"name":"In Progress"}},
            {"id":"21","name":"Done","to":{"name":"Done"}}
        ]}"#;
        let http = MockHttp::new()
            .with("rest/api/3/issue/PROJ-1/transitions", 200, list)
            .with_method("POST", "rest/api/3/issue/PROJ-1/transitions", 204, "");
        let mock = Arc::new(http);
        let client = JiraRestClient::new(mock.clone(), cred());
        client
            .transition_ticket("PROJ-1", "in progress")
            .await
            .unwrap();
        let posts: Vec<_> = mock.calls().into_iter().filter(|c| c.0 == "POST").collect();
        assert_eq!(posts.len(), 1);
        let body: Value = serde_json::from_str(posts[0].2.as_deref().unwrap()).unwrap();
        assert_eq!(
            body["transition"]["id"], "11",
            "matched to.name 'In Progress'"
        );
    }

    #[tokio::test]
    async fn transition_falls_back_to_transition_name() {
        // No `to.name` matches "To Do", but a transition is *named* "To Do".
        let list = r#"{"transitions":[
            {"id":"31","name":"To Do","to":{"name":"Backlog"}}
        ]}"#;
        let http = MockHttp::new()
            .with("rest/api/3/issue/PROJ-1/transitions", 200, list)
            .with_method("POST", "rest/api/3/issue/PROJ-1/transitions", 204, "");
        let mock = Arc::new(http);
        let client = JiraRestClient::new(mock.clone(), cred());
        client.transition_ticket("PROJ-1", "To Do").await.unwrap();
        let body: Value = serde_json::from_str(
            mock.calls()
                .into_iter()
                .find(|c| c.0 == "POST")
                .unwrap()
                .2
                .as_deref()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(body["transition"]["id"], "31");
    }

    #[tokio::test]
    async fn transition_falls_back_to_status_category_when_names_are_localized() {
        // French Jira: the API localizes default status names ("Terminé(e)"),
        // so a literal "Done" can't match by to.name or transition name — but
        // `statusCategory.key` is always "done". The category fallback must
        // still resolve the right transition.
        let list = r#"{"transitions":[
            {"id":"11","name":"A faire","to":{"name":"À faire","statusCategory":{"key":"new"}}},
            {"id":"21","name":"En cours","to":{"name":"En cours","statusCategory":{"key":"indeterminate"}}},
            {"id":"41","name":"Terminé","to":{"name":"Terminé(e)","statusCategory":{"key":"done"}}}
        ]}"#;
        let http = MockHttp::new()
            .with("rest/api/3/issue/PROJ-1/transitions", 200, list)
            .with_method("POST", "rest/api/3/issue/PROJ-1/transitions", 204, "");
        let mock = Arc::new(http);
        let client = JiraRestClient::new(mock.clone(), cred());
        client.transition_ticket("PROJ-1", "Done").await.unwrap();
        let body: Value = serde_json::from_str(
            mock.calls()
                .into_iter()
                .find(|c| c.0 == "POST")
                .unwrap()
                .2
                .as_deref()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            body["transition"]["id"], "41",
            "matched the done-category status despite the localized French name"
        );
    }

    #[tokio::test]
    async fn transition_no_match_is_transition_failed_not_auth() {
        let list = r#"{"transitions":[{"id":"11","name":"Start","to":{"name":"In Progress"}}]}"#;
        let http = MockHttp::new().with("rest/api/3/issue/PROJ-1/transitions", 200, list);
        let client = JiraRestClient::new(Arc::new(http), cred());
        let err = client
            .transition_ticket("PROJ-1", "Done")
            .await
            .unwrap_err();
        let s = err.to_string();
        assert!(
            s.contains("transition"),
            "expected transition error, got: {s}"
        );
        assert!(
            !s.contains("rejected by Jira"),
            "must NOT be the credential error"
        );
    }

    #[tokio::test]
    async fn update_description_puts_adf_round_trips_text() {
        let http = MockHttp::new().with_method("PUT", "rest/api/3/issue/PROJ-1", 204, "");
        let mock = Arc::new(http);
        let client = JiraRestClient::new(mock.clone(), cred());
        client
            .update_description("PROJ-1", "First para.\n\nSecond para.")
            .await
            .unwrap();
        let body: Value = serde_json::from_str(mock.calls()[0].2.as_deref().unwrap()).unwrap();
        let doc = &body["fields"]["description"];
        assert_eq!(doc["type"], "doc");
        assert_eq!(doc["content"][0]["content"][0]["text"], "First para.");
        assert_eq!(doc["content"][1]["content"][0]["text"], "Second para.");
    }

    // ── 401/403 → CredentialRejected (modal code) ─────────────────────────

    #[tokio::test]
    async fn rest_401_maps_to_credential_rejected() {
        let jql = url_encode(
            "project in (\"P\") AND status = \"To Do\" AND issuetype != Epic ORDER BY rank ASC",
        );
        let path = format!(
            "rest/api/3/search/jql?jql={jql}&maxResults=200&fields=summary,issuetype,status,description"
        );
        let http = MockHttp::new().with(&path, 401, "{}");
        let client = JiraRestClient::new(Arc::new(http), cred());
        let err = client
            .list_todo_tickets_by_rank(&["P".to_string()], "")
            .await
            .unwrap_err();
        assert!(
            matches!(
                err,
                crate::error::TakutoError::Jira(crate::jira::JiraError::CredentialRejected {
                    status: 401
                })
            ),
            "expected CredentialRejected{{401}}, got: {err}"
        );
    }

    #[tokio::test]
    async fn transition_401_on_get_maps_to_credential_rejected() {
        // The transitions GET is the first REST call; a 401 there is a rejected
        // token → CredentialRejected (drives the mark-done modal), not a
        // TransitionFailed.
        let http = MockHttp::new().with("rest/api/3/issue/PROJ-1/transitions", 401, "{}");
        let client = JiraRestClient::new(Arc::new(http), cred());
        let err = client
            .transition_ticket("PROJ-1", "Done")
            .await
            .unwrap_err();
        assert!(
            matches!(
                err,
                crate::error::TakutoError::Jira(crate::jira::JiraError::CredentialRejected {
                    status: 401
                })
            ),
            "expected CredentialRejected{{401}}, got: {err}"
        );
    }

    #[tokio::test]
    async fn rest_403_on_write_maps_to_credential_rejected() {
        let http =
            MockHttp::new().with_method("PUT", "rest/api/3/issue/PROJ-1/assignee", 403, "{}");
        let client = JiraRestClient::new(Arc::new(http), cred());
        let err = client.assign_ticket("PROJ-1").await.unwrap_err();
        assert!(
            matches!(
                err,
                crate::error::TakutoError::Jira(crate::jira::JiraError::CredentialRejected {
                    status: 403
                })
            ),
            "expected CredentialRejected{{403}}, got: {err}"
        );
    }
}
