// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Log-line retention.
//!
//! A simple periodic helper that computes the cutoff timestamp
//! and calls [`crate::db::work_items::purge_log_lines_older_than`].
//! The hourly tokio task lives in `maestro-cli/src/main.rs`; this
//! module owns the unit-testable cutoff math + the single-shot
//! `run_once` helper so the math doesn't have to be re-derived
//! everywhere it might run (CLI command, scheduled runner, tests).

use tracing::{info, warn};

use crate::db::Database;
use crate::db::work_items::purge_log_lines_older_than;

/// Milliseconds in one day. Matches the storage units of
/// `work_item_log_lines.emitted_at`.
const MS_PER_DAY: i64 = 86_400_000;

/// Compute the cutoff `emitted_at` for a given retention period.
/// `retention_days == 0` returns `None`, meaning "retention
/// disabled — keep all rows."
pub fn retention_cutoff_ms(now_ms: i64, retention_days: u32) -> Option<i64> {
    if retention_days == 0 {
        return None;
    }
    Some(now_ms - i64::from(retention_days) * MS_PER_DAY)
}

/// Single retention pass. Logs the deleted-row count at INFO when
/// it's non-zero (the hourly task is otherwise silent — operators
/// want one line per actually-useful run, not one per tick).
pub async fn run_once(db: &Database, now_ms: i64, retention_days: u32) {
    let Some(cutoff) = retention_cutoff_ms(now_ms, retention_days) else {
        return;
    };
    match purge_log_lines_older_than(db.adapter(), cutoff).await {
        Ok(0) => { /* clean DB — stay silent */ }
        Ok(n) => {
            info!(
                deleted = n,
                retention_days = retention_days,
                cutoff_ms = cutoff,
                "Ulog retention purge"
            );
        }
        Err(e) => {
            warn!(
                error = %e,
                retention_days = retention_days,
                "Ulog retention purge failed (will retry next tick)"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::adapter::DbValue;
    use crate::db::work_items::{
        LogLineInsert, LogPaging, LogStream, append_log_lines, fetch_log_lines,
    };

    #[test]
    fn retention_cutoff_ms_zero_disables_retention() {
        assert_eq!(retention_cutoff_ms(1_700_000_000_000, 0), None);
    }

    #[test]
    fn retention_cutoff_ms_subtracts_days() {
        // 7 days @ 86_400_000 ms = 604_800_000 ms.
        assert_eq!(
            retention_cutoff_ms(1_700_000_000_000, 7),
            Some(1_700_000_000_000 - 604_800_000)
        );
    }

    /// `run_once` deletes only lines older than the cutoff. Lines
    /// at or after the cutoff stay.
    #[tokio::test]
    async fn run_once_deletes_only_older_lines() {
        let db = Database::open_in_memory().expect("db");
        db.adapter()
            .execute(
                "INSERT INTO users (id, username, role) VALUES ('u-1', 'a', 'admin')",
                Vec::<DbValue>::new(),
            )
            .await
            .unwrap();
        db.adapter()
            .execute(
                "INSERT INTO work_items (\
                    id, ticket_key, workspace_name, user_id, private, \
                    started_manually, counts_toward_manual_cap, driver_started, \
                    jira_available, state_kind, started_at, created_at, updated_at\
                 ) VALUES ('wf-r', 'T-R', 'ws', 'u-1', 0, 0, 0, 0, 0, 'pending', 100, 100, 100)",
                Vec::<DbValue>::new(),
            )
            .await
            .unwrap();

        // Lines at t=100 (old), t=200 (old), t=500 (new).
        append_log_lines(
            db.adapter(),
            &[
                LogLineInsert {
                    work_item_id: "wf-r".into(),
                    step_id: None,
                    stream: LogStream::Stdout,
                    content: "old-1".into(),
                    emitted_at: 100,
                },
                LogLineInsert {
                    work_item_id: "wf-r".into(),
                    step_id: None,
                    stream: LogStream::Stdout,
                    content: "old-2".into(),
                    emitted_at: 200,
                },
                LogLineInsert {
                    work_item_id: "wf-r".into(),
                    step_id: None,
                    stream: LogStream::Stdout,
                    content: "new-1".into(),
                    emitted_at: 500,
                },
            ],
        )
        .await
        .unwrap();

        // now_ms = 500 + 1 day's worth means anything with
        // emitted_at < 500 should disappear at retention_days=0
        // — wait, 0 means disabled. We want a cutoff at 400, so:
        //   now_ms = 400 + 1*MS_PER_DAY = 86_400_400
        //   cutoff = now_ms - 1*MS_PER_DAY = 400
        // Lines emitted_at < 400 → deleted. So old-1 (100) and
        // old-2 (200) go; new-1 (500) stays.
        run_once(&db, 400 + MS_PER_DAY, 1).await;

        let remaining = fetch_log_lines(db.adapter(), "wf-r", LogPaging::default())
            .await
            .unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].content, "new-1");
    }

    /// `run_once` with `retention_days = 0` is a clean no-op —
    /// nothing is deleted. Operators turning retention off must
    /// not lose data.
    #[tokio::test]
    async fn run_once_with_retention_disabled_is_noop() {
        let db = Database::open_in_memory().expect("db");
        db.adapter()
            .execute(
                "INSERT INTO users (id, username, role) VALUES ('u-1', 'a', 'admin')",
                Vec::<DbValue>::new(),
            )
            .await
            .unwrap();
        db.adapter()
            .execute(
                "INSERT INTO work_items (\
                    id, ticket_key, workspace_name, user_id, private, \
                    started_manually, counts_toward_manual_cap, driver_started, \
                    jira_available, state_kind, started_at, created_at, updated_at\
                 ) VALUES ('wf-r', 'T-R', 'ws', 'u-1', 0, 0, 0, 0, 0, 'pending', 100, 100, 100)",
                Vec::<DbValue>::new(),
            )
            .await
            .unwrap();
        append_log_lines(
            db.adapter(),
            &[LogLineInsert {
                work_item_id: "wf-r".into(),
                step_id: None,
                stream: LogStream::Stdout,
                content: "keep me".into(),
                emitted_at: 100,
            }],
        )
        .await
        .unwrap();

        run_once(&db, i64::MAX, 0).await;

        let remaining = fetch_log_lines(db.adapter(), "wf-r", LogPaging::default())
            .await
            .unwrap();
        assert_eq!(remaining.len(), 1);
    }
}
