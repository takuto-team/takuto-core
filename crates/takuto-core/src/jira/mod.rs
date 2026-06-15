// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

pub mod adf_markdown;
mod browse_url;
pub mod client;
pub mod error;
pub mod poller;
pub mod rest;
pub mod source;

pub use browse_url::ticket_browse_url;
pub use error::JiraError;
pub use rest::{
    DbBackedJiraSourceFactory, JiraAccount, JiraHttp, JiraRestClient, JiraRestCredential,
    JiraValidationError, RealJiraHttp, resolve_rest_credential,
    validate as validate_jira_credential,
};
pub use source::{RealJiraSourceFactory, TicketLister, TicketListerFactory, TicketReader};
