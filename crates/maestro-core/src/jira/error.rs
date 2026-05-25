// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Typed errors for the Jira subsystem.
//!
//! Sub-enum that captures every distinct failure mode produced inside
//! `crates/maestro-core/src/jira/` and the five `actions/{real,dry_run}.rs`
//! near-duplicate `acli` invocations. Lifted from
//! `MaestroError::Jira(String)` per the 2026-05-24 typed-errors-jira spec —
//! every variant cites the call site it replaces so the migration commits can
//! be traced back.
//!
//! Wired into the workspace error envelope via
//! `MaestroError::Jira(#[from] JiraError)` so existing `?` propagation across
//! `Result<T, MaestroError>` boundaries keeps working unchanged.
//!
//! # Foreign `#[from]` policy — none
//!
//! `serde_json::Error` is referenced from four variants
//! (`ParseTicketJson`, `ParseTicketListJson`, `ParseTicketDetailJson`,
//! `ParseLinkedItemJson`); only one `#[from]` per source type is legal under
//! `thiserror`, and each parse site carries distinct context (`key` vs.
//! structural-kind), so all four use `#[source]` + explicit `.map_err(...)`.
//! Mirrors the `GitHubAppError` policy for `jsonwebtoken::errors::Error` /
//! `chrono::format::ParseError`. No `std::io::Error` (acli failures
//! `?`-propagate through `MaestroError::Io` / `Command`). No HTTP-client type
//! (acli is shelled out).

/// Failures originating inside the Jira subsystem. Public for matching, but
/// callers should generally just `?`-propagate into a `MaestroError`.
#[derive(Debug, thiserror::Error)]
pub enum JiraError {
    /// `jira/client.rs:198` — `get_ticket_description_preview` invariant: the
    /// key's project prefix is not in `[jira] project_keys`.
    #[error("ticket {key} is not in configured [jira] project_keys")]
    TicketNotInConfiguredProjects { key: String },

    // ── acli subprocess failures (output.success() == false) ────────────────
    /// `jira/client.rs:178` — `list_todo_tickets_by_rank`.
    #[error("acli list To Do tickets failed: {stderr}")]
    ListTodoFailed { stderr: String },

    /// `jira/client.rs:216` — `get_ticket_description_preview`.
    #[error("acli load ticket {key} failed: {stderr}")]
    GetDescriptionPreviewFailed { key: String, stderr: String },

    /// `jira/client.rs:269` + `actions/real.rs:179` + `actions/dry_run.rs:83`.
    #[error("acli get ticket details for {key} failed: {stderr}")]
    GetDetailsFailed { key: String, stderr: String },

    /// `jira/client.rs:313` + `actions/real.rs:100`.
    #[error("acli assign ticket {key} failed: {stderr}")]
    AssignFailed { key: String, stderr: String },

    /// `jira/client.rs:336` + `actions/real.rs:153`.
    #[error("acli unassign ticket {key} failed: {stderr}")]
    UnassignFailed { key: String, stderr: String },

    /// `jira/client.rs:360` + `actions/real.rs:127`.
    #[error("acli transition ticket {key} to {status} failed: {stderr}")]
    TransitionFailed {
        key: String,
        status: String,
        stderr: String,
    },

    /// `jira/client.rs:384`.
    #[error("acli update description for {key} failed: {stderr}")]
    UpdateDescriptionFailed { key: String, stderr: String },

    /// `jira/client.rs:406`.
    #[error("acli get linked item {key} failed: {stderr}")]
    GetLinkedItemFailed { key: String, stderr: String },

    // ── serde_json parsing of acli `--json` output ──────────────────────────
    /// `jira/client.rs:223` — `get_ticket_description_preview`. Carries `key`
    /// because the parse site already has it bound.
    #[error("failed to parse acli ticket JSON for {key}")]
    ParseTicketJson {
        key: String,
        #[source]
        source: serde_json::Error,
    },

    /// `jira/client.rs:421` — `parse_ticket_list` (no key in scope).
    #[error("failed to parse acli ticket list JSON")]
    ParseTicketListJson {
        #[source]
        source: serde_json::Error,
    },

    /// `jira/client.rs:467` — `parse_ticket_detail` (no key bound at parse).
    #[error("failed to parse acli ticket detail JSON")]
    ParseTicketDetailJson {
        #[source]
        source: serde_json::Error,
    },

    /// `jira/client.rs:589` — `parse_linked_item` (no key bound at parse).
    #[error("failed to parse acli linked item JSON")]
    ParseLinkedItemJson {
        #[source]
        source: serde_json::Error,
    },
}
