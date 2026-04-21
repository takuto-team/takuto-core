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
}
