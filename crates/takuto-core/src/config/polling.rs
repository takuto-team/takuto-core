// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Item-polling filter helper. Polling policy (which projects/labels/keywords
//! a repository polls, which flow it auto-starts, parallel-item caps) is now
//! **per-user, per-repository** — see `db::user_repo_polling_settings` and
//! `/api/me/polling-settings`. The deployment-global `[polling]` config section
//! was removed; deployment-wide limits live in `[general]`. This module retains
//! only the shared keyword-match helper used by both pollers.

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
