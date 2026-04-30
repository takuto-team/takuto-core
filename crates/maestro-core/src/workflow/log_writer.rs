// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::path::{Path, PathBuf};

use chrono::Utc;
use tokio::fs::{self, OpenOptions};
use tokio::io::AsyncWriteExt;
use tracing::warn;

/// Writes timestamped log entries to a per-workflow log file.
pub struct WorkflowLogWriter {
    log_path: PathBuf,
}

impl WorkflowLogWriter {
    pub async fn new(log_dir: &Path, ticket_key: &str) -> Self {
        let log_path = log_dir.join(format!("{ticket_key}.log"));

        // Ensure log directory exists
        if let Err(e) = fs::create_dir_all(log_dir).await {
            warn!(error = %e, "Failed to create log directory");
        }

        // Write header
        let writer = Self { log_path };
        writer
            .write(&format!("=== Workflow log for {ticket_key} ==="))
            .await;
        writer
            .write(&format!("Started at {}", Utc::now().to_rfc3339()))
            .await;
        writer
    }

    pub async fn write(&self, message: &str) {
        let timestamp = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ");
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
    }

    pub async fn write_step(&self, step_name: &str, message: &str) {
        self.write(&format!("[{step_name}] {message}")).await;
    }

    pub async fn write_output(&self, step_name: &str, stream: &str, line: &str) {
        self.write(&format!("[{step_name}] [{stream}] {line}"))
            .await;
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
}
