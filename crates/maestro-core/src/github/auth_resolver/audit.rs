// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Audit helpers for the `auth_resolver` module.
//!
//! Holds the "first use in the last minute" debounce predicate and the
//! `credential_audit` row-write helper that the resolver fires from
//! `materialise_user_pat`.
//!
//! Both are extracted unchanged from the legacy `auth_resolver.rs` — the
//! debounce window and the `(event="used", outcome="ok")` row layout are
//! a stable part of the security audit contract.

use crate::db::credential_audit::{self, CredentialAuditKind};
use crate::db::github_credentials;
use crate::db::Database;

/// "First use in the last minute" debounce. Returns `true` if we should
/// emit an audit row for this use. `last_used` is the previous
/// `last_validated_at` string we co-opt as a debounce flag.
pub(super) fn should_audit_first_use(last_used: Option<&str>) -> bool {
    let Some(prev) = last_used else {
        return true;
    };
    // Anything we can't parse as RFC-3339 we audit (conservatively re-emit).
    let prev_dt = match chrono::DateTime::parse_from_rfc3339(prev) {
        Ok(dt) => dt.with_timezone(&chrono::Utc),
        Err(_) => return true,
    };
    chrono::Utc::now() - prev_dt > chrono::Duration::seconds(60)
}

/// Write a `credential_audit` "used" row for a successful first-in-window
/// PAT use, and bump `last_validated_at` (which the resolver co-opts as the
/// debounce flag).
///
/// Plan-11 step 3 cluster B: both DAOs migrated to the agnostic adapter.
/// Each call opens its own short transaction under the hood (sqlx-pool
/// connections serialize via SQLite's single-writer lock anyway), so the
/// two writes are no longer literally co-committed in one transaction —
/// audit emit is best-effort observability and a partial-success window
/// is acceptable (the touch precedes the audit log, so a failure between
/// them leaves the debounce flag in the safe state).
pub(super) async fn record_first_use(db: &Database, user_id: &str) {
    let now = chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    let adapter = db.adapter();
    let _ = github_credentials::touch_last_validated_adapter(adapter, user_id, &now).await;
    let _ = credential_audit::log(
        adapter,
        user_id,
        Some(user_id),
        CredentialAuditKind::GithubPat,
        None,
        "used",
        "ok",
        None,
    )
    .await;
}

