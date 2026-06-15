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

use super::client::{JiraTicket, LinkedItem};
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

#[async_trait]
impl JiraHttp for RealJiraHttp {
    async fn get(
        &self,
        cred: &JiraRestCredential,
        path: &str,
    ) -> std::result::Result<JiraHttpResponse, String> {
        let url = format!(
            "{}/{}",
            cred.site.trim_end_matches('/'),
            path.trim_start_matches('/')
        );
        // `--user email:token` makes curl build the Basic header itself; the
        // token is passed as a process argument (like the gh `-H` path), never
        // interpolated into a shell string.
        let userpass = format!("{}:{}", cred.email, cred.token);
        let write_out = format!("\n{STATUS_MARKER}%{{http_code}}");
        let args = [
            "-sS",
            "--max-time",
            "30",
            "--connect-timeout",
            "10",
            "-H",
            "Accept: application/json",
            "--user",
            &userpass,
            "-w",
            &write_out,
            &url,
        ];
        let cwd = std::env::temp_dir();
        let output = process::run_command("curl", &args, &cwd, CancellationToken::new())
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
        if resp.status != 200 {
            return Err(crate::jira::JiraError::ListTodoFailed {
                stderr: format!("Jira REST {path} returned {}", resp.status),
            }
            .into());
        }
        serde_json::from_str(&resp.body)
            .map_err(|source| crate::jira::JiraError::ParseTicketListJson { source }.into())
    }
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
        let path = format!(
            "rest/api/3/search?jql={encoded}&maxResults=50&fields=summary,issuetype,status,description"
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
    use std::sync::Mutex;

    struct MockHttp {
        responses: Mutex<std::collections::HashMap<String, JiraHttpResponse>>,
    }

    impl MockHttp {
        fn new() -> Self {
            Self {
                responses: Mutex::new(std::collections::HashMap::new()),
            }
        }
        fn with(self, path: &str, status: u16, body: &str) -> Self {
            self.responses.lock().unwrap().insert(
                path.to_string(),
                JiraHttpResponse {
                    status,
                    body: body.to_string(),
                },
            );
            self
        }
    }

    #[async_trait]
    impl JiraHttp for MockHttp {
        async fn get(
            &self,
            _cred: &JiraRestCredential,
            path: &str,
        ) -> std::result::Result<JiraHttpResponse, String> {
            self.responses
                .lock()
                .unwrap()
                .get(path)
                .cloned()
                .ok_or_else(|| format!("no canned response for {path}"))
        }
    }

    fn cred() -> JiraRestCredential {
        JiraRestCredential {
            site: "https://acme.atlassian.net".into(),
            email: "a@acme.com".into(),
            token: "tok".into(),
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
            "rest/api/3/search?jql={jql}&maxResults=50&fields=summary,issuetype,status,description"
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
}
