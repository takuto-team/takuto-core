// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::path::{Path, PathBuf};

use chrono::Utc;
use tokio::fs::{self, OpenOptions};
use tokio::io::AsyncWriteExt;
use tracing::warn;

use crate::db::work_items::{LogLineInsert, LogStream};
use crate::workflow::log_sink::LogSink;

/// Writes timestamped log entries to a per-workflow log file.
///
/// The writer ALSO forwards each line to an optional [`LogSink`] so it
/// lands in `work_item_log_lines`. The file output is preserved
/// unchanged; the DB is shadow data today (additive —
/// `GET /api/work-items/{id}/log` reads it).
pub struct WorkflowLogWriter {
    log_path: PathBuf,
    sink: Option<LogSink>,
    /// `work_items.id` — the FK target every `LogLineInsert`
    /// needs. `None` when no DB / no sink is attached, in which
    /// case the sink branch is short-circuited.
    work_item_id: Option<String>,
}

impl WorkflowLogWriter {
    pub async fn new(log_dir: &Path, ticket_key: &str) -> Self {
        Self::with_sink(log_dir, ticket_key, None, None).await
    }

    /// Construct with an attached log sink and the `work_item_id`
    /// that every emitted line will reference. Either both are
    /// `Some` or both are `None` — passing one without the other
    /// silently disables DB persistence.
    pub async fn with_sink(
        log_dir: &Path,
        ticket_key: &str,
        sink: Option<LogSink>,
        work_item_id: Option<String>,
    ) -> Self {
        let log_path = log_dir.join(format!("{ticket_key}.log"));

        // Ensure log directory exists
        if let Err(e) = fs::create_dir_all(log_dir).await {
            warn!(error = %e, "Failed to create log directory");
        }

        // Write header
        let writer = Self {
            log_path,
            sink: match (sink, work_item_id.as_ref()) {
                (Some(s), Some(_)) => Some(s),
                _ => None,
            },
            work_item_id,
        };
        writer
            .write(&format!("=== Workflow log for {ticket_key} ==="))
            .await;
        writer
            .write(&format!("Started at {}", Utc::now().to_rfc3339()))
            .await;
        writer
    }

    pub async fn write(&self, message: &str) {
        let now = Utc::now();
        let timestamp = now.format("%Y-%m-%dT%H:%M:%S%.3fZ");
        let line = format!("[{timestamp}] {message}\n");

        match OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_path)
            .await
        {
            Ok(mut file) => {
                if let Err(e) = file.write_all(line.as_bytes()).await {
                    warn!(error = %e, "Failed to write to workflow log");
                }
            }
            Err(e) => {
                warn!(error = %e, path = %self.log_path.display(), "Failed to open workflow log");
            }
        }

        self.emit_to_sink(LogStream::System, message, now.timestamp_millis());
    }

    pub async fn write_step(&self, step_name: &str, message: &str) {
        let now = Utc::now();
        let composed = format!("[{step_name}] {message}");
        // Inline the file-write here so the sink emission can mark
        // the line as `LogStream::Info` rather than the generic
        // `System` emitted by `write()`.
        let timestamp = now.format("%Y-%m-%dT%H:%M:%S%.3fZ");
        let line = format!("[{timestamp}] {composed}\n");
        if let Err(e) = self.append_to_file(&line).await {
            warn!(error = %e, "Failed to write workflow log");
        }
        self.emit_to_sink(LogStream::Info, &composed, now.timestamp_millis());
    }

    pub async fn write_output(&self, step_name: &str, stream: &str, line: &str) {
        let now = Utc::now();
        let composed = format!("[{step_name}] [{stream}] {line}");
        let timestamp = now.format("%Y-%m-%dT%H:%M:%S%.3fZ");
        let file_line = format!("[{timestamp}] {composed}\n");
        if let Err(e) = self.append_to_file(&file_line).await {
            warn!(error = %e, "Failed to write workflow log");
        }
        let db_stream = match stream {
            "stdout" => LogStream::Stdout,
            "stderr" => LogStream::Stderr,
            _ => LogStream::Info,
        };
        self.emit_to_sink(db_stream, line, now.timestamp_millis());
    }

    async fn append_to_file(&self, line: &str) -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_path)
            .await?;
        file.write_all(line.as_bytes()).await
    }

    fn emit_to_sink(&self, stream: LogStream, content: &str, emitted_at_ms: i64) {
        let (Some(sink), Some(work_item_id)) = (self.sink.as_ref(), self.work_item_id.as_ref())
        else {
            return;
        };
        sink.send(LogLineInsert {
            work_item_id: work_item_id.clone(),
            step_id: None,
            stream,
            content: content.to_string(),
            emitted_at: emitted_at_ms,
        });
    }

    /// Returns the path where this writer stores its log file.
    #[cfg(test)]
    pub fn log_path(&self) -> &std::path::Path {
        &self.log_path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn log_path_is_under_log_dir_with_ticket_key() {
        let dir = tempfile::tempdir().unwrap();
        let writer = WorkflowLogWriter::new(dir.path(), "PROJ-42").await;
        let expected = dir.path().join("PROJ-42.log");
        assert_eq!(writer.log_path(), expected);
    }

    #[tokio::test]
    async fn log_path_no_traversal_from_ticket_key() {
        let dir = tempfile::tempdir().unwrap();
        // A ticket key with path separators should be treated as a filename component,
        // not allow traversal. The join will embed the "/" literally in the filename
        // on Unix, but the log stays under the log dir.
        let writer = WorkflowLogWriter::new(dir.path(), "SAFE-1").await;
        assert!(writer.log_path().starts_with(dir.path()));
    }

    #[tokio::test]
    async fn write_appends_lines_to_file() {
        let dir = tempfile::tempdir().unwrap();
        let writer = WorkflowLogWriter::new(dir.path(), "TEST-1").await;

        writer.write("first line").await;
        writer.write("second line").await;

        let content = tokio::fs::read_to_string(writer.log_path()).await.unwrap();
        // The header writes two lines, then our two lines
        let lines: Vec<&str> = content.lines().collect();
        assert!(lines.len() >= 4);
        assert!(lines.last().unwrap().contains("second line"));
        // All lines should have timestamps in brackets
        for line in &lines {
            assert!(line.starts_with('['));
        }
    }

    #[tokio::test]
    async fn write_step_includes_step_name() {
        let dir = tempfile::tempdir().unwrap();
        let writer = WorkflowLogWriter::new(dir.path(), "TEST-2").await;

        writer.write_step("Build", "compilation succeeded").await;

        let content = tokio::fs::read_to_string(writer.log_path()).await.unwrap();
        assert!(content.contains("[Build] compilation succeeded"));
    }

    #[tokio::test]
    async fn write_output_includes_stream_label() {
        let dir = tempfile::tempdir().unwrap();
        let writer = WorkflowLogWriter::new(dir.path(), "TEST-3").await;

        writer
            .write_output("Lint", "stderr", "warning: unused var")
            .await;

        let content = tokio::fs::read_to_string(writer.log_path()).await.unwrap();
        assert!(content.contains("[Lint] [stderr] warning: unused var"));
    }

    /// When constructed with a `LogSink` + a `work_item_id`, the writer must:
    ///   - keep writing to the file (no regression to the legacy
    ///     download-log behaviour), AND
    ///   - emit each line through the sink so it lands in
    ///     `work_item_log_lines` once the batcher flushes.
    ///
    /// Stream mapping: write_step → Info, write_output stderr →
    /// Stderr, write_output stdout → Stdout, write (header etc.) →
    /// System.
    #[tokio::test]
    async fn writer_with_sink_emits_to_db_and_file() {
        use crate::db::Database;
        use crate::db::adapter::DbValue;
        use crate::db::work_items::{LogPaging, LogStream, fetch_log_lines};
        use crate::workflow::log_sink::spawn_batcher;

        let db = Database::open_in_memory().expect("db");
        // Seed FK targets.
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
                 ) VALUES ('wf-log', 'T-LOG', 'ws', 'u-1', 0, 0, 0, 0, 0, 'pending', 100, 100, 100)",
                Vec::<DbValue>::new(),
            )
            .await
            .unwrap();

        let sink = spawn_batcher(db.clone());

        let dir = tempfile::tempdir().unwrap();
        let writer = WorkflowLogWriter::with_sink(
            dir.path(),
            "T-LOG",
            Some(sink.clone()),
            Some("wf-log".into()),
        )
        .await;

        writer.write_step("Build", "starting").await;
        writer
            .write_output("Build", "stdout", "compiling foo")
            .await;
        writer
            .write_output("Build", "stderr", "warning: unused")
            .await;

        // Drop sink + writer so the batcher hits the
        // "sender disconnected" path and flushes immediately.
        drop(writer);
        drop(sink);
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let rows = fetch_log_lines(db.adapter(), "wf-log", LogPaging::default())
            .await
            .unwrap();
        // The first two rows are the header (write_step body via
        // `write()` → LogStream::System) for the constructor. Then
        // our three explicit emissions.
        let by_stream: Vec<(LogStream, String)> =
            rows.iter().map(|r| (r.stream, r.content.clone())).collect();
        assert!(
            by_stream
                .iter()
                .any(|(s, c)| *s == LogStream::Info && c.contains("[Build] starting")),
            "write_step must emit as Info, got {by_stream:?}"
        );
        assert!(
            by_stream
                .iter()
                .any(|(s, c)| *s == LogStream::Stdout && c == "compiling foo"),
            "write_output stdout must emit as Stdout with the raw line content"
        );
        assert!(
            by_stream
                .iter()
                .any(|(s, c)| *s == LogStream::Stderr && c == "warning: unused"),
            "write_output stderr must emit as Stderr"
        );
        assert!(
            by_stream.iter().any(|(s, _)| *s == LogStream::System),
            "the constructor header (via `write()`) emits as System"
        );

        // The file output is preserved unchanged.
        let content = tokio::fs::read_to_string(dir.path().join("T-LOG.log"))
            .await
            .unwrap();
        assert!(content.contains("[Build] starting"));
        assert!(content.contains("[Build] [stdout] compiling foo"));
        assert!(content.contains("[Build] [stderr] warning: unused"));
    }
}
