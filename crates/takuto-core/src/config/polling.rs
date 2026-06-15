// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Admin-tunable item-polling policy: which discovered work items the Jira /
//! GitHub pollers auto-add, which flow they auto-start, and how many items may
//! run in parallel. Stored in `config.toml` under `[polling]`, admin-gated via
//! `PUT /api/config/polling`, and read live by the pollers each cycle.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PollingConfig {
    /// Flow slug to auto-start for each polled item. Empty = start all
    /// dependency-free flows (legacy behavior).
    #[serde(default)]
    pub auto_start_flow: String,
    /// Maximum number of items occupying a concurrency slot at once. `0` =
    /// unlimited (the legacy ceilings still apply).
    #[serde(default)]
    pub max_parallel_items: u32,
    /// When `true`, `max_parallel_items` is enforced per workflow owner;
    /// `false` enforces it globally.
    #[serde(default)]
    pub max_parallel_per_user: bool,
    #[serde(default)]
    pub jira: PollingJiraFilter,
    #[serde(default)]
    pub github: PollingGitHubFilter,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PollingJiraFilter {
    /// Case-insensitive ANY-substring match against the ticket summary. Empty
    /// list = no filter.
    #[serde(default)]
    pub summary_keywords: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PollingGitHubFilter {
    /// Exact label membership, ANY match. Empty list = no filter.
    #[serde(default)]
    pub labels: Vec<String>,
    /// Case-insensitive ANY-substring match against the issue title. Empty
    /// list = no filter.
    #[serde(default)]
    pub title_keywords: Vec<String>,
}

/// Returns `true` when `haystack` contains any of `keywords` as a
/// case-insensitive substring. An empty keyword list means "no filter" and
/// matches everything. Each keyword is trimmed before comparison; blank
/// keywords are skipped (validation rejects them at config-write time, but
/// this stays robust against a hand-edited `config.toml`).
pub fn matches_any_keyword(haystack: &str, keywords: &[String]) -> bool {
    if keywords.is_empty() {
        return true;
    }
    let haystack_lower = haystack.to_lowercase();
    keywords.iter().any(|kw| {
        let needle = kw.trim();
        !needle.is_empty() && haystack_lower.contains(&needle.to_lowercase())
    })
}
