// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! `LogSink` + `LogBatcher`.
//!
//! A cheaply-cloneable sink that workflow log writers can send
//! `LogLineInsert`s through without blocking the engine on DB
//! writes. The batcher task drains the channel, accumulates up to
//! [`BATCH_SIZE`] lines or [`FLUSH_INTERVAL`] (whichever first),
//! and flushes via [`crate::db::work_items::append_log_lines`].
//!
//! Contract:
//!   - The sink is best-effort. If the batcher task is gone,
//!     `send` returns silently — log persistence is shadow data,
//!     not the truth-of-record. The file writer keeps recording
//!     the same lines independently.
//!   - The batcher runs until the last `LogSink` is dropped, then
//!     flushes any remaining lines and exits. This makes it safe
//!     to keep an `Arc<LogSink>` alongside the engine and rely on
//!     normal shutdown to clean up.
//!
//! No retention policy here — the retention runner lives as a separate
//! background task in `maestro-cli`.

use std::time::Duration;

use tokio::sync::mpsc;
use tracing::warn;

use crate::db::Database;
use crate::db::work_items::{LogLineInsert, append_log_lines};

/// Maximum lines accumulated before a flush is forced.
pub const BATCH_SIZE: usize = 50;
/// Maximum age of the oldest buffered line before flushing.
pub const FLUSH_INTERVAL: Duration = Duration::from_millis(100);

/// Cheaply-cloneable handle for sending log lines to the batcher.
/// Holds an unbounded `Sender`; cloning is `O(1)` (just bumps the
/// internal refcount) and a flooded sender does not block the
/// caller — `try_send` records the drop at WARN.
#[derive(Clone)]
pub struct LogSink {
    tx: mpsc::UnboundedSender<LogLineInsert>,
}

impl LogSink {
    /// Send a line. Returns immediately. If the receiver is gone
    /// (batcher task exited), the line is dropped silently.
    pub fn send(&self, line: LogLineInsert) {
        if let Err(e) = self.tx.send(line) {
            // The batcher exited — log persistence is best-effort,
            // and the file writer captured the same line anyway.
            tracing::trace!(
                error = %e,
                "log batcher receiver gone; dropping line"
            );
        }
    }
}

/// Spawn the batcher task. Returns a [`LogSink`] handle the engine
/// can clone into every workflow log writer it builds.
///
/// The task exits when all `LogSink` clones are dropped.
pub fn spawn_batcher(db: Database) -> LogSink {
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(run_batcher(db, rx));
    LogSink { tx }
}

async fn run_batcher(db: Database, mut rx: mpsc::UnboundedReceiver<LogLineInsert>) {
    let mut buffer: Vec<LogLineInsert> = Vec::with_capacity(BATCH_SIZE);
    loop {
        // Block waiting for the FIRST line in the next batch. This
        // is the idle path — when no logs are arriving we yield the
        // task entirely rather than polling on a tick.
        let Some(first) = rx.recv().await else {
            // All senders dropped — flush remainder + exit.
            flush(&db, &mut buffer).await;
            return;
        };
        buffer.push(first);

        // Drain whatever else is sitting in the channel right now —
        // when the engine is chatty we hit the BATCH_SIZE cap on
        // the first round-trip without paying for any timer wakeup.
        while buffer.len() < BATCH_SIZE {
            match rx.try_recv() {
                Ok(line) => buffer.push(line),
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    flush(&db, &mut buffer).await;
                    return;
                }
            }
        }

        // If we filled the batch synchronously, skip the timer wait.
        if buffer.len() < BATCH_SIZE {
            // Give late stragglers up to FLUSH_INTERVAL to land
            // before we commit the batch. Bail early on either
            // (a) reaching BATCH_SIZE, (b) timer elapsing, or
            // (c) channel close.
            let deadline = tokio::time::sleep(FLUSH_INTERVAL);
            tokio::pin!(deadline);
            loop {
                tokio::select! {
                    biased;
                    _ = &mut deadline => break,
                    maybe_line = rx.recv() => match maybe_line {
                        Some(line) => {
                            buffer.push(line);
                            if buffer.len() >= BATCH_SIZE { break; }
                        }
                        None => {
                            // All senders dropped — flush and exit.
                            flush(&db, &mut buffer).await;
                            return;
                        }
                    }
                }
            }
        }

        flush(&db, &mut buffer).await;
    }
}

async fn flush(db: &Database, buffer: &mut Vec<LogLineInsert>) {
    if buffer.is_empty() {
        return;
    }
    if let Err(e) = append_log_lines(db.adapter(), buffer).await {
        warn!(
            count = buffer.len(),
            error = %e,
            "Ulog batcher flush failed (lines dropped — shadow-write only)"
        );
    }
    buffer.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::adapter::DbValue;
    use crate::db::work_items::{LogStream, fetch_log_lines, LogPaging};

    async fn seeded_db() -> Database {
        let db = Database::open_in_memory().expect("open db");
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
                 ) VALUES ('wf-1', 'T-1', 'ws', 'u-1', 0, 0, 0, 0, 0, 'pending', 100, 100, 100)",
                Vec::<DbValue>::new(),
            )
            .await
            .unwrap();
        db
    }

    fn line(seq: i64) -> LogLineInsert {
        LogLineInsert {
            work_item_id: "wf-1".into(),
            step_id: None,
            stream: LogStream::Stdout,
            content: format!("line-{seq}"),
            emitted_at: seq,
        }
    }

    /// Send fewer than BATCH_SIZE lines, drop the sink, and confirm
    /// the batcher drains the buffer before exiting.
    #[tokio::test]
    async fn batcher_flushes_remainder_on_sender_drop() {
        let db = seeded_db().await;
        let sink = spawn_batcher(db.clone());
        for i in 0..5 {
            sink.send(line(i));
        }
        drop(sink);
        // Give the spawned task a tick to finalise.
        tokio::time::sleep(Duration::from_millis(200)).await;

        let rows = fetch_log_lines(db.adapter(), "wf-1", LogPaging::default())
            .await
            .unwrap();
        assert_eq!(rows.len(), 5, "all buffered lines must flush at shutdown");
        for (i, r) in rows.iter().enumerate() {
            assert_eq!(r.content, format!("line-{i}"));
        }
    }

    /// Sending BATCH_SIZE lines triggers a size-bound flush without
    /// waiting for FLUSH_INTERVAL.
    #[tokio::test]
    async fn batcher_flushes_when_batch_size_reached() {
        let db = seeded_db().await;
        let sink = spawn_batcher(db.clone());
        for i in 0..(BATCH_SIZE as i64) {
            sink.send(line(i));
        }
        // FLUSH_INTERVAL is 100ms; with size-bound flush we should
        // see rows much sooner. Allow up to 50ms to schedule.
        tokio::time::sleep(Duration::from_millis(50)).await;

        let rows = fetch_log_lines(db.adapter(), "wf-1", LogPaging::default())
            .await
            .unwrap();
        assert_eq!(
            rows.len(),
            BATCH_SIZE,
            "size-bound flush must complete before timer elapses"
        );

        drop(sink);
    }

    /// A few lines flush after FLUSH_INTERVAL even without filling
    /// the batch.
    #[tokio::test]
    async fn batcher_flushes_on_timer_when_batch_partial() {
        let db = seeded_db().await;
        let sink = spawn_batcher(db.clone());
        sink.send(line(1));
        sink.send(line(2));
        // Wait long enough for the timer to fire AND the flush to
        // complete: FLUSH_INTERVAL (100ms) + DB round-trip.
        tokio::time::sleep(FLUSH_INTERVAL + Duration::from_millis(100)).await;

        let rows = fetch_log_lines(db.adapter(), "wf-1", LogPaging::default())
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);

        drop(sink);
    }
}
